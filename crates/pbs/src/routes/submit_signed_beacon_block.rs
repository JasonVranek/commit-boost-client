use axum::{body::Bytes, extract::State, http::HeaderMap, response::IntoResponse};
use cb_common::wire::{
    BodyDeserializeError, EncodingType, content_type_encoding, get_user_agent,
    require_consensus_version_header,
};
use reqwest::StatusCode;
use tracing::{debug, info};

use crate::{
    PbsStateGuard,
    constants::SUBMIT_SIGNED_BEACON_BLOCK_ENDPOINT_TAG,
    error::PbsClientError,
    state::{BuilderApiState, PbsState},
    utils::{
        epbs_base_send_headers, join_detached_sends, post_ssz_expect_accepted,
        record_beacon_status, record_request_failure,
    },
};

/// POST /eth/v1/builder/beacon_blocks (submitSignedBeaconBlock).
/// `Eth-Consensus-Version` is required (spec PR #165) and names the block's
/// fork. CB is a blind pipe: it forwards the block bytes to every builder
/// WITHOUT decoding them, because block validity is the builder's job
/// (builder-specs: an invalid block MUST be rejected by the builder).
pub async fn handle_submit_signed_beacon_block<S: BuilderApiState>(
    State(state): State<PbsStateGuard<S>>,
    req_headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, PbsClientError> {
    let state = state.read().clone();

    match submit_signed_beacon_block(body, req_headers, state).await {
        Ok(()) => {
            record_beacon_status("202", SUBMIT_SIGNED_BEACON_BLOCK_ENDPOINT_TAG);
            Ok(StatusCode::ACCEPTED.into_response())
        }
        Err(err) => Err(record_request_failure(err, SUBMIT_SIGNED_BEACON_BLOCK_ENDPOINT_TAG)),
    }
}

/// Forwards the SSZ block bytes to every configured builder. CB keeps no
/// auction state, so it forwards to all relays to improve inclusion, additive
/// to the beacon node's own p2p gossip. Ok(()) means at least one builder
/// accepted with a 202.
pub async fn submit_signed_beacon_block<S: BuilderApiState>(
    body: Bytes,
    req_headers: HeaderMap,
    state: PbsState<S>,
) -> Result<(), PbsClientError> {
    let ua = get_user_agent(&req_headers);

    // Eth-Consensus-Version is spec-required here. Only Gloas is accepted,
    // which is the fork the outbound headers already carry.
    require_consensus_version_header(&req_headers)?;

    // The outbound is always SSZ, so the reveal must be SSZ; a JSON or
    // otherwise non-SSZ reveal would be forwarded mislabeled as octet-stream
    // and fail opaquely downstream, so reject it up front with 415. The
    // Content-Type defaults to JSON when absent (builder-specs), so an
    // unlabeled reveal is treated as JSON and rejected, not assumed SSZ.
    if body.is_empty() {
        return Err(BodyDeserializeError::MissingBody.into());
    }
    if content_type_encoding(&req_headers)? != EncodingType::Ssz {
        return Err(BodyDeserializeError::UnsupportedMediaType.into());
    }
    let out_body = body;

    info!(ua, "new request");

    let send_headers = epbs_base_send_headers(&req_headers)?;
    let timeout_ms = state.pbs_config().timeout_get_payload_ms;
    let relays = state.all_relays();
    let results = join_detached_sends(relays.iter().map(|relay| {
        let (relay, body, headers) = (relay.clone(), out_body.clone(), send_headers.clone());
        async move {
            // Every builder implements SSZ for this new endpoint, so the block is
            // forwarded in SSZ (the fork travels in Eth-Consensus-Version).
            let url = relay.submit_signed_beacon_block_url()?;
            post_ssz_expect_accepted(
                &relay,
                url,
                body,
                headers,
                timeout_ms,
                SUBMIT_SIGNED_BEACON_BLOCK_ENDPOINT_TAG,
            )
            .await?;
            Ok(())
        }
    }))
    .await;
    let accepted = results
        .into_iter()
        .zip(relays.iter())
        .filter(|(res, relay)| match res {
            Ok(()) => true,
            Err(err) => {
                debug!(relay_id = relay.id.as_ref(), %err, "builder did not accept the block; only the winner accepts");
                false
            }
        })
        .count();

    // Only the winner accepts, so one 202 across the broadcast is success
    if accepted == 0 {
        return Err(PbsClientError::NoBuilderResponse);
    }
    info!(accepted, addressed = relays.len(), "signed beacon block submitted");
    Ok(())
}
