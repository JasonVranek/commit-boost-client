---
description: Run Commit-Boost for ePBS proposals after the Gloas fork
---

# ePBS

:::info Unreleased
ePBS support is not in a Commit-Boost release yet; it ships from v0.12.0-rc1.
:::

From the Gloas fork, a proposer commits to a builder's signed execution payload bid instead of a blinded header, and the winning builder publishes the payload itself. Commit-Boost serves the builder-API endpoints the beacon node calls for this and forwards each call to builders. On this page a builder is whatever a relay entry's `url` points at.

## What changes at the fork

For a Gloas proposal the beacon node calls three endpoints on Commit-Boost:

| Endpoint | What the beacon node sends | What Commit-Boost does |
|---|---|---|
| `POST /eth/v1/builder/execution_payload_bid/{slot}/{parent_hash}/{parent_root}/{proposer_pubkey}` | A bid request for the proposal slot | Asks the [addressed](#routing-by-auth-data) builder and returns its bid with `200`, or `204` if it has none |
| `POST /eth/v1/builder/builder_preferences/{proposer_pubkey}` | The proposer's `max_execution_payment` for one builder, ahead of the slot | Forwards it to the addressed builder and returns `202` when it accepts, or its `400` or `401` |
| `POST /eth/v1/builder/beacon_blocks` | The signed beacon block that commits to the winning bid | Forwards it to every relay in `[[relays]]` and `[[mux.relays]]` and returns `202` if one accepts, as only the winning builder does |

Every request must carry `Eth-Consensus-Version: gloas`; a missing header or another fork is a `400`. The endpoints are always served, so Commit-Boost needs no fork schedule.

Validator registrations, `status` calls and relay entries are unchanged. On ePBS bids Commit-Boost ignores `skip_sigverify`, `min_bid_eth` and `extra_validation_enabled`, since the beacon node checks the bid against the on-chain builder registry and applies the `min_bid` from its builder config. It also ignores `timeout_get_header_ms`, `late_in_slot_time_ms` and the timing-games options (see [Timing](#timing)), and asks a `get_header = "stream"` relay over plain HTTP.

## Setup

The Commit-Boost config lists the builders as relay entries, in `[[relays]]` or in the `[[mux.relays]]` of the mux that lists the proposer's key. ePBS adds these options:

| Option | Where | Default | Effect |
|---|---|---|---|
| `proposer_deadline_buffer_ms` | `[pbs]` | `50` | Time kept back from the beacon node's deadline (see [Timing](#timing)). Must be under one slot, `12000` on mainnet |
| `expected_auth_data` | relay entry | unset | Hex bytes. The relay matches only auth data equal to these bytes, not its hostname |

### Validator builder config

Each validator key's builder config, set through the validator client's keymanager API, tells the beacon node which URLs to ask for bids and with which auth data. Write one entry per builder under `builders`: its `url` is Commit-Boost's own URL, and its `auth_data` is the hex of the builder's hostname bytes, or of the value agreed with the builder, which then also goes in that relay's `expected_auth_data`. A key without builder config sends the default auth data of Commit-Boost's own URL, which matches no relay, so every bid request gets `400`.

The validator client must implement the keymanager API's builder config endpoint ([keymanager-APIs #88](https://github.com/ethereum/keymanager-APIs/pull/88)). At the time of writing the Lodestar, Prysm, Lighthouse and Nimbus validator clients implement it in their development builds; Teku and Grandine do not yet. Write a key's config with `POST /eth/v1/validator/{pubkey}/builder_config`, authenticated with the keymanager API's bearer token. The body replaces the key's config in full, and the validator client answers `202` once it is stored. `min_bid` and `builder_boost_factor` are optional; at the top level they apply to every entry, and to p2p bids, unless an entry sets its own. A top-level `min_bid` also floors the bids that come through Commit-Boost, so a value above your builders' bids sends every proposal to p2p bids or a local block. An entry without `max_execution_payment` gets the validator client's default, which differs between clients (Lodestar stores `0`, Nimbus the maximum), and Lodestar accepts a nonzero value only when started with `--allowDangerousTrustedPayments`.

For `url = "https://0xa1ce...@builder-a.example.com"` in the Commit-Boost config, served at `http://cb.example.com:18550`, with the keymanager API at `$KEYMANAGER_URL`, its token in `$TOKEN` and the validator key in `$PUBKEY`:

```bash
curl -X POST "$KEYMANAGER_URL/eth/v1/validator/$PUBKEY/builder_config" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "builders": [
      {
        "url": "http://cb.example.com:18550",
        "auth_data": "0x6275696c6465722d612e6578616d706c652e636f6d",
        "builder_pubkeys": []
      }
    ]
  }'
```

`echo 0x$(printf builder-a.example.com | xxd -p | tr -d '\n')` prints the `auth_data` hex. An empty `builder_pubkeys` accepts any builder: the pubkey in a relay URL identifies the relay, not its bid-signing key.

## Routing by auth data

Bid and preferences requests carry a `SignedBuilderRequestAuth` signed with the validator's key. Its `slot` is the proposal slot and its `data` names the builder: a value agreed with the builder out of band or, by default, the builder's hostname. The [builder specs](https://github.com/ethereum/builder-specs/blob/main/specs/gloas/validator.md#default-auth-data) define the default as the lowercase hostname of the builder's URL, without scheme, port or path, with an IPv6 address in brackets, such as `[::1]`. Commit-Boost answers `400` to an empty `data` and to a bid request whose `slot` differs from the path's, and forwards the message unchanged for the builder to verify.

Each request goes to the builder its `data` names, chosen from the relays that serve the proposer: the relays of the `[[mux]]` that lists the proposer's key, otherwise `[[relays]]`. Among those:

1. A relay with `expected_auth_data` set matches when `data` equals those bytes exactly.
2. Any other relay matches when `data` is the hostname of its `url`, byte for byte.

Only the first matching relay, in config order, is asked. Relays on one host share its hostname, so to address them separately, set each relay's `expected_auth_data` to a value agreed with its builder. When no relay matches, Commit-Boost answers `400`.

## Bid selection

A bid is dropped when it does not decode or is labeled with a fork other than Gloas.

The beacon node applies `min_bid` and `builder_boost_factor` from the builder config: it rejects a bid below `min_bid`, weights the rest by `builder_boost_factor`, and compares the result with its local block.

## Timing

Bid requests without `Date-Milliseconds` and a non-zero `X-Timeout-Ms` get `400`. The beacon node's deadline is `Date-Milliseconds + X-Timeout-Ms`, but no more than `X-Timeout-Ms` from when Commit-Boost handles the request and no more than one slot. Commit-Boost keeps `proposer_deadline_buffer_ms` of it for returning the winning bid and for the beacon node's own work, and gives each relay one request that may take the rest; if nothing is left, it returns `204` without asking anyone. `timeout_get_header_ms` and `late_in_slot_time_ms` are not used, nor their `[[mux]]` overrides, so a late request is served while the deadline allows.

Builder preferences are sent with `timeout_register_validator_ms`, and the signed block with `timeout_get_payload_ms`. Commit-Boost passes the signed block's bytes to builders unparsed, so the block must be SSZ (`Content-Type: application/octet-stream`); anything else gets `415`.

## Metrics

With [metrics](./running/metrics.md) enabled, the ePBS endpoints use the `endpoint` labels `get_execution_payload_bid`, `submit_builder_preferences` and `submit_signed_beacon_block`.

| Question | Series |
|---|---|
| What did Commit-Boost answer the beacon node? | `cb_pbs_beacon_node_status_code_total`: `200` or `204` for a bid request, `202` for preferences and the signed block, `4xx` for a rejected request, `500` when no builder accepted preferences or the signed block |
| What did each builder answer? | `cb_pbs_relay_status_code_total`, by `relay_id`: the builder's HTTP status, or `555` when no response arrived (timeout, DNS or connection failure) |
| How fast are builders? | `cb_pbs_relay_latency`, by `relay_id` |
| What are builders bidding? | `cb_pbs_relay_header_value` (the bid's `value` in Gwei, without the execution payment) and `cb_pbs_relay_last_slot`, by `relay_id`, from the bid each relay contributes to a request |
| Which bids were dropped? | `cb_pbs_relay_invalid_response_total`, by `reason`: `wrong_fork` (see [Bid selection](#bid-selection)). The relay's own status still counts in `cb_pbs_relay_status_code_total`. A bid that fails to decode is only logged |

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Bid requests get `400` "auth.message.data does not match the value agreed with this builder" | The auth data matches no relay serving the key. Usually the key has no builder config, so its auth data is Commit-Boost's own hostname | Write the key's [builder config](#validator-builder-config), or add the builder it names as a relay entry for the key |
| A builder answers bid requests with `400` (`cb_pbs_relay_status_code_total{endpoint="get_execution_payload_bid",http_status_code="400"}`) | The builder compares auth data byte for byte and expects something other than its hostname | Set the relay's `expected_auth_data` and the key's builder config `auth_data` to the bytes the builder expects |
| The signed block gets `415` | The beacon node sends it as JSON | Configure the beacon node to send SSZ |
| Commit-Boost returns `200` but the beacon node builds locally | The beacon node rejected the bid, or valued its local block higher after `builder_boost_factor` | Compare the bid with the key's `min_bid` and `builder_boost_factor` |
