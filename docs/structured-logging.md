# Structured Logging

The `structured_log` module (host builds only — excluded from `wasm`) gives the
operational workflows a consistent, machine-readable log format so host events
can be correlated with contract activity and operational conditions. It replaces
ad-hoc `eprintln!` diagnostics with typed records that serialise to JSON lines.

## Log payload schema

Every entry is a `LogRecord` and serialises to a single JSON line via
`LogRecord::to_json_line()`:

```json
{"ts":1712345678,"seq":3,"level":"warn","event":"webhook.delivery_attempt_failed","fields":{"endpoint_url":"https://anchor.example.com/hook","attempt":2,"status":503,"error":"HTTP 503"}}
```

| Key      | Type   | Meaning |
|----------|--------|---------|
| `ts`     | number | Unix timestamp in seconds, injected by the caller (the crate is `no_std` and has no clock; on-chain wrappers use `env.ledger().timestamp()`). |
| `seq`    | number | Monotonic per-logger sequence number, for ordering entries that share a timestamp. Keeps incrementing across drains. |
| `level`  | string | `debug`, `info`, `warn`, or `error`. |
| `event`  | string | Canonical dotted event name (`workflow.event`); see the catalog below. Constants live in `structured_log::events`. |
| `fields` | object | Event-specific context. Values are typed (strings, integers, booleans) and keep insertion order, so output is deterministic and diffable. |

Key order in the envelope is fixed (`ts`, `seq`, `level`, `event`, `fields`).
String values are JSON-escaped.

## Using the logger

```rust
use anchorkit::structured_log::{LogLevel, StructuredLogger};

let logger = StructuredLogger::new()          // Info and above, 1024-record buffer
    .with_min_level(LogLevel::Debug)          // optional
    .with_capacity(4096);                     // optional; 0 = unlimited

// ... pass &logger to any *_logged workflow API ...

for line in logger.drain_json_lines() {       // ship wherever you like
    // e.g. write to a file, stdout, or a log shipper
}
```

The logger performs no I/O of its own: records accumulate in a bounded
in-memory buffer (oldest evicted first; evictions are counted in
`logger.dropped()`) and the host decides where to ship them. With the `std`
feature, `logger.flush_to_stderr()` drains the buffer to stderr as JSON lines.
The logger is not thread-safe by design — the crate is `no_std` and callers
own their concurrency story.

Instrumentation is opt-in and behaviour-preserving: every workflow keeps its
plain function, and a `*_logged` variant takes `&StructuredLogger` as its last
parameter and behaves identically otherwise.

## Event catalog

### Attestor registration

Registration itself runs on-chain (`register_attestor` emits an
`attestor.added` contract event and an admin-audit entry). The host wraps its
submission in `log_attestor_registration(&logger, ts, attestor, issuer, || ...)`
to get correlated off-chain logs:

| Event | Level | Fields |
|-------|-------|--------|
| `attestor.registration_started`   | info  | `attestor`, `sep10_issuer` |
| `attestor.registration_succeeded` | info  | `attestor` |
| `attestor.registration_failed`    | error | `attestor`, `error` |

### Transaction status polling

`StreamingTransactionMonitor::run_logged(...)`:

| Event | Level | Fields |
|-------|-------|--------|
| `txstatus.monitor_started`     | info  | `transaction_id`, `poll_interval_ms` |
| `txstatus.poll_error`          | warn  | `transaction_id`, `consecutive_errors`, `error` |
| `txstatus.state_changed`       | info  | `transaction_id`, `from`, `to` (`ts` is the transition timestamp) |
| `txstatus.more_info_available` | info  | `transaction_id`, `url` |
| `txstatus.completed`           | info  | `transaction_id`, `stellar_tx_id` |
| `txstatus.failed`              | error | `transaction_id`, `reason` |

### Webhook delivery

`deliver_webhook_logged(...)`:

| Event | Level | Fields |
|-------|-------|--------|
| `webhook.delivery_started`        | info  | `endpoint_url`, `dlq_key`, `payload_bytes`, `max_attempts`, `signed` |
| `webhook.delivery_attempt_failed` | warn  | `endpoint_url`, `attempt`, `status` (0 = transport failure), `error` |
| `webhook.delivery_succeeded`      | info  | `endpoint_url`, `attempts` |
| `webhook.delivery_failed`         | error | `endpoint_url`, `attempts`, `last_status`, `last_error` |
| `webhook.dlq_entry_added`         | warn  | `dlq_key`, `dlq_depth` |

### Cache governance

`propose_logged`, `endorse_logged`, `execute_logged`, `set_policy_set_logged`,
`enforce_write_policy_logged`, `enforce_invalidation_policy_logged` (host
builds only; on-chain builds keep the plain functions and the contract event
stream). Timestamps come from `env.ledger().timestamp()`:

| Event | Level | Fields |
|-------|-------|--------|
| `cache.proposal_created`        | info | `proposal_id`, `anchor`, `proposer`, `ledger` |
| `cache.proposal_endorsed`       | info | `proposal_id`, `endorser`, `endorsement_count` |
| `cache.proposal_endorse_failed` | warn | `proposal_id`, `error` |
| `cache.proposal_executed`       | info | `proposal_id`, `anchor` |
| `cache.proposal_execute_failed` | warn | `proposal_id`, `error` |
| `cache.policy_updated`          | info | `metadata_max_ttl_seconds`, `capabilities_max_ttl_seconds`, `other_max_ttl_seconds` |
| `cache.policy_rejected`         | warn | `error` |
| `cache.ttl_clamped`             | warn | `entry_type`, `requested_ttl_seconds`, `effective_ttl_seconds` |
| `cache.invalidation_denied`     | warn | `entry_type` |

Policy-enforcement wrappers log only noteworthy conditions (a clamped TTL, a
denied invalidation); in-band operations stay silent.

## Sample output

A webhook delivery that exhausts two attempts and dead-letters:

```json
{"ts":1712345678,"seq":0,"level":"info","event":"webhook.delivery_started","fields":{"endpoint_url":"https://anchor.example.com/hook","dlq_key":"anchor-1","payload_bytes":118,"max_attempts":2,"signed":true}}
{"ts":1712345678,"seq":1,"level":"warn","event":"webhook.delivery_attempt_failed","fields":{"endpoint_url":"https://anchor.example.com/hook","attempt":1,"status":503,"error":"HTTP 503"}}
{"ts":1712345679,"seq":2,"level":"warn","event":"webhook.delivery_attempt_failed","fields":{"endpoint_url":"https://anchor.example.com/hook","attempt":2,"status":503,"error":"HTTP 503"}}
{"ts":1712345679,"seq":3,"level":"error","event":"webhook.delivery_failed","fields":{"endpoint_url":"https://anchor.example.com/hook","attempts":2,"last_status":503,"last_error":"HTTP 503"}}
{"ts":1712345679,"seq":4,"level":"warn","event":"webhook.dlq_entry_added","fields":{"dlq_key":"anchor-1","dlq_depth":1}}
```

## Validation

- Unit tests in `src/structured_log.rs` cover the JSON envelope, escaping,
  level filtering, buffer eviction, and sequence numbering.
- Integration tests in `tests/structured_logging_tests.rs` assert the exact
  event sequences and context fields emitted by each workflow, that the
  envelope is identical across modules, and that each `*_logged` wrapper is
  behaviour-preserving with respect to its plain counterpart.

Run them with:

```
cargo test --lib structured_log
cargo test --test structured_logging_tests
```
