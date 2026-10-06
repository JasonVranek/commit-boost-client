use std::{
    future::Future,
    time::{Duration, Instant},
};

use cb_common::{
    pbs::{
        ForkName, RelayClient, SignedBuilderRequestAuth, error::PbsError, relay_matches_auth_data,
    },
    types::BlsPublicKey,
    wire::{
        CONSENSUS_VERSION_HEADER, EncodingType, get_user_agent_with_version,
        safe_read_http_response,
    },
};
use futures::future::join_all;
use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};
use tracing::{Instrument, debug, error, warn};
use url::Url;

use crate::{
    constants::{MAX_SIZE_DEFAULT, TIMEOUT_ERROR_CODE_STR},
    error::PbsClientError,
    metrics::{BEACON_NODE_STATUS, RELAY_INVALID_RESPONSE, RELAY_LATENCY, RELAY_STATUS_CODE},
};

/// Sends one already-built relay request, recording the per-relay metrics
/// shared by all three ePBS endpoints, and returns the response and its latency
/// so the caller can read/decode the body itself. `tag` is the per-endpoint
/// metric label. Callers build their own `RequestBuilder` because the requests
/// legitimately differ (bid sets a per-call timeout and timing headers).
pub(crate) async fn send_to_relay(
    req: reqwest::RequestBuilder,
    relay: &RelayClient,
    tag: &str,
) -> Result<(reqwest::Response, Duration), PbsError> {
    let start_request = Instant::now();
    let res = match req.send().await {
        Ok(res) => res,
        Err(err) => {
            RELAY_STATUS_CODE.with_label_values(&[TIMEOUT_ERROR_CODE_STR, tag, &relay.id]).inc();
            return Err(err.into());
        }
    };

    let request_latency = start_request.elapsed();
    RELAY_LATENCY.with_label_values(&[tag, &relay.id]).observe(request_latency.as_secs_f64());

    let code = res.status();
    RELAY_STATUS_CODE.with_label_values(&[code.as_str(), tag, &relay.id]).inc();

    Ok((res, request_latency))
}

/// Count a request-rejection in `BEACON_NODE_STATUS` before it short-circuits
/// the handler, so rejected requests show up as 4xx in the endpoint counter.
pub(crate) fn record_client_error(
    err: impl Into<PbsClientError>,
    endpoint: &str,
) -> PbsClientError {
    let err = err.into();
    BEACON_NODE_STATUS.with_label_values(&[err.status_code().as_str(), endpoint]).inc();
    err
}

pub(crate) fn record_beacon_status(code: &str, endpoint: &str) {
    BEACON_NODE_STATUS.with_label_values(&[code, endpoint]).inc();
}

/// Logs and counts a failed ePBS request before it is returned to the beacon
/// node. A 4xx is the caller's fault, not CB's: only a 5xx is an error.
pub(crate) fn record_request_failure(err: PbsClientError, endpoint: &str) -> PbsClientError {
    if err.status_code().is_server_error() {
        error!(%err, "{endpoint} failed");
    } else {
        warn!(%err, "{endpoint} failed");
    }
    record_beacon_status(err.status_code().as_str(), endpoint);
    err
}

/// Fans `sends` out on detached tasks and waits for all of them: a BN
/// disconnect must not cancel in-flight writes mid-fan-out, leaving some
/// builders with the data and others without.
pub(crate) async fn join_detached_sends<F>(
    sends: impl IntoIterator<Item = F>,
) -> Vec<Result<(), PbsError>>
where
    F: Future<Output = Result<(), PbsError>> + Send + 'static,
{
    let handles: Vec<_> =
        sends.into_iter().map(|send| tokio::spawn(send.in_current_span())).collect();
    join_all(handles)
        .await
        .into_iter()
        .map(|joined| joined.unwrap_or_else(|err| Err(PbsError::TokioJoinError(err))))
        .collect()
}

pub(crate) fn log_mux_selection(
    maybe_mux_id: Option<&str>,
    relay_count: usize,
    pubkey: &BlsPublicKey,
) {
    match maybe_mux_id {
        Some(mux_id) => {
            debug!(mux_id, relays = relay_count, pubkey = %pubkey, "using mux config")
        }
        None => debug!(relays = relay_count, pubkey = %pubkey, "using default config"),
    }
}

/// Count a relay response that CB rejected during validation, by reason (see
/// `RELAY_INVALID_RESPONSE` for why this is a separate signal from the relay's
/// HTTP status).
pub(crate) fn record_invalid_relay_response(reason: &str, endpoint: &str, relay_id: &str) {
    RELAY_INVALID_RESPONSE.with_label_values(&[reason, endpoint, relay_id]).inc();
}

/// POSTs an SSZ body to a builder and enforces the ePBS write-endpoint
/// contract: 202 Accepted is the only success. The response body is read (and
/// capped) then discarded - a builder is untrusted and must not stream an
/// unbounded error body into memory or the logs. Returns the request latency.
/// Shared by `submitBuilderPreferences` and `submitSignedBeaconBlock`.
pub(crate) async fn post_ssz_expect_accepted(
    relay: &RelayClient,
    url: Url,
    body: impl Into<reqwest::Body>,
    headers: HeaderMap,
    timeout_ms: u64,
    tag: &str,
) -> Result<Duration, PbsError> {
    let req = relay
        .client
        .post(url)
        .timeout(Duration::from_millis(timeout_ms))
        .headers(headers)
        .header(CONTENT_TYPE, EncodingType::Ssz.content_type_header().clone())
        .body(body);
    let (res, latency) = send_to_relay(req, relay, tag).await?;
    let code = res.status();
    safe_read_http_response(res, MAX_SIZE_DEFAULT).await?;
    if code != StatusCode::ACCEPTED {
        return Err(PbsError::RelayResponse {
            error_msg: "expected 202".to_string(),
            code: code.as_u16(),
        });
    }
    Ok(latency)
}

/// Base outbound headers shared by the ePBS endpoints: the versioned
/// `User-Agent` and `Eth-Consensus-Version`. All three relay hops send SSZ
/// bodies of fork-versioned wire types, so the builder needs the fork header,
/// and all three are Gloas-only. Callers add their endpoint-specific headers
/// (bid adds `Accept` and the timing headers).
pub(crate) fn epbs_base_send_headers(req_headers: &HeaderMap) -> Result<HeaderMap, PbsClientError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        get_user_agent_with_version(req_headers).map_err(|_| PbsClientError::Internal)?,
    );
    headers.insert(
        CONSENSUS_VERSION_HEADER,
        HeaderValue::from_str(&ForkName::Gloas.to_string())
            .expect("fork name is always a valid header value"),
    );
    Ok(headers)
}

const GAS_LIMIT_ADJUSTMENT_FACTOR: u64 = 1024;
const GAS_LIMIT_MINIMUM: u64 = 5_000;

/// Validates the gas limit against the parent gas limit, according to the
/// execution spec https://github.com/ethereum/execution-specs/blob/98d6ddaaa709a2b7d0cd642f4cfcdadc8c0808e1/src/ethereum/cancun/fork.py#L1118-L1154
pub fn check_gas_limit(gas_limit: u64, parent_gas_limit: u64) -> bool {
    let max_adjustment_delta = parent_gas_limit / GAS_LIMIT_ADJUSTMENT_FACTOR;
    if gas_limit >= parent_gas_limit + max_adjustment_delta {
        return false;
    }

    if gas_limit <= parent_gas_limit - max_adjustment_delta {
        return false;
    }

    if gas_limit < GAS_LIMIT_MINIMUM {
        return false;
    }

    true
}

/// A zero-length `auth.message.data` is invalid per builder-specs
/// `types/gloas/request_auth.yaml` (pattern `{1,4096}`, "A zero-length `data`
/// is invalid"). Shared by both ePBS request-auth validators.
pub(crate) fn validate_auth_data(auth: &SignedBuilderRequestAuth) -> Result<(), PbsClientError> {
    if auth.message.data.is_empty() {
        warn!("auth data is empty");
        return Err(PbsClientError::EmptyAuthData);
    }

    Ok(())
}

/// Selects the relay an ePBS request is addressed to: the first configured
/// relay whose auth data matches the caller's `auth.message.data` (see
/// [`relay_matches_auth_data`]).
pub(crate) fn match_relay_by_auth_data<'a>(
    relays: &'a [RelayClient],
    received_data: &[u8],
) -> Option<&'a RelayClient> {
    relays.iter().find(|relay| relay_matches_auth_data(&relay.config, received_data))
}

/// Resolves the one relay an ePBS request is sent to: the first configured
/// relay whose auth data matches (see [`match_relay_by_auth_data`]). The
/// beacon node keeps each (url, auth data) pair unique, so a request names one
/// builder; comparing bids across builders is the beacon node's job across its
/// per-entry calls. Auth data matching no configured relay is a 400. Shared by
/// the bid and preferences endpoints so their routing cannot diverge.
pub(crate) fn resolve_addressed_relay(
    relays: &[RelayClient],
    auth_data: &[u8],
) -> Result<RelayClient, PbsClientError> {
    match match_relay_by_auth_data(relays, auth_data) {
        Some(relay) => Ok(relay.clone()),
        None => {
            warn!(
                auth_data = %String::from_utf8_lossy(auth_data),
                "auth data matches no configured relay"
            );
            Err(PbsClientError::AuthDataMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use cb_common::{
        config::{GetHeaderTransport, RelayConfig},
        pbs::RelayEntry,
        types::BlsSecretKey,
    };

    use super::*;

    fn test_relay(url: &str, expected_auth_data: Option<&[u8]>) -> RelayClient {
        let entry = RelayEntry {
            id: url.to_string(),
            pubkey: BlsSecretKey::random().public_key().into(),
            url: Url::parse(url).unwrap(),
        };
        let config = RelayConfig {
            entry,
            id: None,
            headers: None,
            get_params: None,
            get_header: GetHeaderTransport::Http,
            enable_timing_games: false,
            target_first_request_ms: None,
            frequency_get_header_ms: None,
            validator_registration_batch_size: None,
            expected_auth_data: expected_auth_data.map(|d| d.to_vec().into()),
        };
        RelayClient::new(config).unwrap()
    }

    // With nothing agreed out of band, `data` is the builder's hostname
    // (builder-specs `get_default_auth_data`), compared byte for byte
    #[test]
    fn match_relay_by_default_auth_data() {
        let relays = vec![
            test_relay("https://0xdeadbeef@builder-a.example.com", None),
            test_relay("https://builder-b.example.com:8443/eth", None),
            test_relay("http://[::1]:18550", None),
        ];
        let host = |data: &[u8]| -> Option<String> {
            match_relay_by_auth_data(&relays, data)
                .map(|relay| relay.config.entry.url.host_str().unwrap().to_string())
        };
        assert_eq!(host(b"builder-a.example.com").as_deref(), Some("builder-a.example.com"));
        assert_eq!(host(b"builder-b.example.com").as_deref(), Some("builder-b.example.com"));
        assert_eq!(host(b"[::1]").as_deref(), Some("[::1]"));
        assert!(host(b"Builder-A.example.com").is_none());
        assert!(host(b"builder-a.example.com:443").is_none());
        // A URL is not the default form
        assert!(host(b"https://builder-a.example.com").is_none());
    }

    #[test]
    fn match_relay_expected_auth_data_replaces_the_default() {
        let relays = vec![test_relay("https://builder.example.com", Some(&[0xaa]))];
        assert!(match_relay_by_auth_data(&relays, b"builder.example.com").is_none());
    }
}
