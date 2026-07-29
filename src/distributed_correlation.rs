//! Distributed request correlation (#684).
//!
//! In multi-process deployments a single logical request can flow through
//! several components (HTTP gateway → anchor service → Soroban node →
//! webhook dispatcher). Without a shared identifier, correlating log lines
//! from all those components is extremely difficult.
//!
//! This module extends the existing [`TraceContext`] with a
//! [`CorrelationContext`] that bundles a W3C-compatible `trace_id` with
//! additional propagation metadata (originating service, hop count, custom
//! baggage) so all components can tag their logs with the same root
//! correlation identifier.
//!
//! # Wire format
//!
//! Correlation metadata is propagated via HTTP headers:
//!
//! | Header | Content |
//! |--------|---------|
//! | `X-Correlation-Id` | Root correlation ID (32 hex chars, stable across the entire request tree) |
//! | `X-Origin-Service` | Name of the service that originated this request |
//! | `X-Hop-Count` | Number of service hops so far (decimal integer) |
//! | `X-Baggage` | Optional key=value pairs separated by `,` |
//!
//! # Relationship to `TraceContext`
//!
//! [`TraceContext`] (`traceparent` / W3C Trace Context) handles span-level
//! tracing within a single request. [`CorrelationContext`] carries the
//! higher-level correlation ID that ties together multiple microservice
//! invocations and their individual traces. They are complementary: a
//! correlation header suite (`X-Correlation-Id`) can be attached alongside
//! `traceparent` headers in the same outbound request.
//!
//! # Example
//!
//! ```rust
//! use anchorkit::distributed_correlation::CorrelationContext;
//!
//! // Originating service stamps the first context.
//! let ctx = CorrelationContext::new("gateway", "txn-001");
//! assert_eq!(ctx.hop_count(), 0);
//!
//! // Downstream service receives and propagates.
//! let headers = ctx.to_headers();
//! let downstream = CorrelationContext::from_headers(&headers).unwrap();
//! let next = downstream.next_hop("anchor-service");
//!
//! assert_eq!(next.correlation_id(), ctx.correlation_id());
//! assert_eq!(next.hop_count(), 1);
//! assert_eq!(next.origin_service(), "gateway");
//! ```

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Header name constants
// ---------------------------------------------------------------------------

/// `X-Correlation-Id` header name.
pub const CORRELATION_ID_HEADER: &str = "X-Correlation-Id";
/// `X-Origin-Service` header name.
pub const ORIGIN_SERVICE_HEADER: &str = "X-Origin-Service";
/// `X-Hop-Count` header name.
pub const HOP_COUNT_HEADER: &str = "X-Hop-Count";
/// `X-Baggage` header name.
pub const BAGGAGE_HEADER: &str = "X-Baggage";

// ---------------------------------------------------------------------------
// CorrelationError
// ---------------------------------------------------------------------------

/// Errors that can arise when parsing a [`CorrelationContext`] from headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorrelationError {
    /// The `X-Correlation-Id` header was missing.
    MissingCorrelationId,
    /// The `X-Correlation-Id` value was not a valid 32-char lowercase hex string.
    InvalidCorrelationId(String),
    /// The `X-Hop-Count` value could not be parsed as an integer.
    InvalidHopCount(String),
}

impl core::fmt::Display for CorrelationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CorrelationError::MissingCorrelationId => {
                write!(f, "missing X-Correlation-Id header")
            }
            CorrelationError::InvalidCorrelationId(v) => {
                write!(f, "invalid X-Correlation-Id value: '{v}'")
            }
            CorrelationError::InvalidHopCount(v) => {
                write!(f, "invalid X-Hop-Count value: '{v}'")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CorrelationContext
// ---------------------------------------------------------------------------

/// Distributed request correlation context.
///
/// Carries a stable root correlation ID plus metadata that allows every
/// service hop to tag its logs with the same identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrelationContext {
    /// Stable root correlation ID (32 lowercase hex chars).
    correlation_id: String,
    /// Name of the service that originated the request.
    origin_service: String,
    /// Current service's own name (changes at each hop).
    current_service: String,
    /// Number of service hops since origination (0 = this is the originator).
    hop_count: u32,
    /// Optional key=value baggage entries propagated downstream.
    baggage: BTreeMap<String, String>,
}

impl CorrelationContext {
    /// Create a new root correlation context for `originating_service`.
    ///
    /// The correlation ID is derived deterministically from `seed` (typically
    /// a transaction ID or idempotency key) so the same seed always produces
    /// the same ID, making tests deterministic.
    pub fn new(originating_service: impl Into<String>, seed: &str) -> Self {
        let id = derive_correlation_id(seed);
        let svc = originating_service.into();
        CorrelationContext {
            correlation_id: id,
            origin_service: svc.clone(),
            current_service: svc,
            hop_count: 0,
            baggage: BTreeMap::new(),
        }
    }

    /// Create a context with a caller-supplied correlation ID (e.g. when
    /// forwarding an ID generated by an external gateway).
    ///
    /// Returns [`CorrelationError::InvalidCorrelationId`] when `id` is not a
    /// 32-char lowercase hex string.
    pub fn with_id(
        originating_service: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, CorrelationError> {
        let id = id.into();
        validate_correlation_id(&id)?;
        let svc = originating_service.into();
        Ok(CorrelationContext {
            correlation_id: id,
            origin_service: svc.clone(),
            current_service: svc,
            hop_count: 0,
            baggage: BTreeMap::new(),
        })
    }

    // ── Accessors ──────────────────────────────────────────────────────────

    /// The stable root correlation ID (32 lowercase hex chars).
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// The service that originated this request.
    pub fn origin_service(&self) -> &str {
        &self.origin_service
    }

    /// The current service's name at this hop.
    pub fn current_service(&self) -> &str {
        &self.current_service
    }

    /// Number of service hops since origination.
    pub fn hop_count(&self) -> u32 {
        self.hop_count
    }

    /// Retrieve a baggage value by key.
    pub fn baggage(&self, key: &str) -> Option<&str> {
        self.baggage.get(key).map(String::as_str)
    }

    // ── Mutation ───────────────────────────────────────────────────────────

    /// Add or overwrite a baggage entry.
    pub fn set_baggage(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.baggage.insert(key.into(), value.into());
        self
    }

    // ── Propagation ───────────────────────────────────────────────────────

    /// Produce a new [`CorrelationContext`] for the next downstream service.
    ///
    /// The correlation ID and origin service are preserved; `current_service`
    /// is updated to `next_service` and `hop_count` is incremented.
    pub fn next_hop(&self, next_service: impl Into<String>) -> Self {
        CorrelationContext {
            correlation_id: self.correlation_id.clone(),
            origin_service: self.origin_service.clone(),
            current_service: next_service.into(),
            hop_count: self.hop_count.saturating_add(1),
            baggage: self.baggage.clone(),
        }
    }

    // ── Header serialisation ───────────────────────────────────────────────

    /// Serialize this context to a list of `(header_name, header_value)` pairs
    /// suitable for attaching to an outbound HTTP request.
    pub fn to_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        headers.push((CORRELATION_ID_HEADER.to_string(), self.correlation_id.clone()));
        headers.push((ORIGIN_SERVICE_HEADER.to_string(), self.origin_service.clone()));
        headers.push((HOP_COUNT_HEADER.to_string(), self.hop_count.to_string()));
        if !self.baggage.is_empty() {
            let baggage_str = self
                .baggage
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            headers.push((BAGGAGE_HEADER.to_string(), baggage_str));
        }
        headers
    }

    /// Deserialize a [`CorrelationContext`] from a slice of
    /// `(header_name, header_value)` pairs.
    ///
    /// The `current_service` defaults to the origin service on ingress; callers
    /// should call [`next_hop`](Self::next_hop) to stamp their own service name.
    pub fn from_headers(headers: &[(String, String)]) -> Result<Self, CorrelationError> {
        let mut correlation_id: Option<String> = None;
        let mut origin_service: Option<String> = None;
        let mut hop_count: u32 = 0;
        let mut baggage: BTreeMap<String, String> = BTreeMap::new();

        for (name, value) in headers {
            let n = name.as_str();
            if n.eq_ignore_ascii_case(CORRELATION_ID_HEADER) {
                correlation_id = Some(value.clone());
            } else if n.eq_ignore_ascii_case(ORIGIN_SERVICE_HEADER) {
                origin_service = Some(value.clone());
            } else if n.eq_ignore_ascii_case(HOP_COUNT_HEADER) {
                hop_count = value.parse::<u32>().map_err(|_| {
                    CorrelationError::InvalidHopCount(value.clone())
                })?;
            } else if n.eq_ignore_ascii_case(BAGGAGE_HEADER) {
                for pair in value.split(',') {
                    let pair = pair.trim();
                    if let Some(pos) = pair.find('=') {
                        let k = pair[..pos].trim().to_string();
                        let v = pair[pos + 1..].trim().to_string();
                        if !k.is_empty() {
                            baggage.insert(k, v);
                        }
                    }
                }
            }
        }

        let id = correlation_id.ok_or(CorrelationError::MissingCorrelationId)?;
        validate_correlation_id(&id)?;
        let origin = origin_service.unwrap_or_default();

        Ok(CorrelationContext {
            current_service: origin.clone(),
            origin_service: origin,
            correlation_id: id,
            hop_count,
            baggage,
        })
    }

    /// Produce a log-friendly summary string.
    ///
    /// ```text
    /// corr=<id> origin=<svc> hop=<n>
    /// ```
    pub fn log_fields(&self) -> String {
        format!(
            "corr={} origin={} current={} hop={}",
            self.correlation_id, self.origin_service, self.current_service, self.hop_count
        )
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Derive a 32-char lowercase hex correlation ID from an arbitrary seed string
/// using SHA-256 (first 16 bytes).
fn derive_correlation_id(seed: &str) -> String {
    sha256_hex_16(seed.as_bytes())
}

/// SHA-256 of `data`, first 16 bytes as lowercase hex (= 32 chars).
fn sha256_hex_16(data: &[u8]) -> String {
    // Minimal SHA-256 using the same approach as trace_context.
    // We rely on the `sha2` crate already present in Cargo.toml.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let result = h.finalize();
    result[..16]
        .iter()
        .fold(String::new(), |mut s, b| {
            s.push(hex_nibble(b >> 4));
            s.push(hex_nibble(b & 0x0f));
            s
        })
}

fn hex_nibble(v: u8) -> char {
    match v {
        0..=9 => (b'0' + v) as char,
        _ => (b'a' + v - 10) as char,
    }
}

/// Validate that `id` is exactly 32 lowercase hex characters.
fn validate_correlation_id(id: &str) -> Result<(), CorrelationError> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(CorrelationError::InvalidCorrelationId(id.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_produces_valid_correlation_id() {
        let ctx = CorrelationContext::new("gateway", "txn-001");
        assert_eq!(ctx.correlation_id().len(), 32);
        assert!(ctx.correlation_id().bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(ctx.hop_count(), 0);
        assert_eq!(ctx.origin_service(), "gateway");
    }

    #[test]
    fn same_seed_is_deterministic() {
        let a = CorrelationContext::new("svc", "txn-001");
        let b = CorrelationContext::new("svc", "txn-001");
        assert_eq!(a.correlation_id(), b.correlation_id());
    }

    #[test]
    fn different_seeds_differ() {
        let a = CorrelationContext::new("svc", "txn-001");
        let b = CorrelationContext::new("svc", "txn-002");
        assert_ne!(a.correlation_id(), b.correlation_id());
    }

    #[test]
    fn next_hop_preserves_correlation_id() {
        let ctx = CorrelationContext::new("gateway", "txn-001");
        let downstream = ctx.next_hop("anchor-service");
        assert_eq!(downstream.correlation_id(), ctx.correlation_id());
        assert_eq!(downstream.origin_service(), "gateway");
        assert_eq!(downstream.current_service(), "anchor-service");
        assert_eq!(downstream.hop_count(), 1);
    }

    #[test]
    fn multi_hop_increments_count() {
        let ctx = CorrelationContext::new("a", "seed");
        let hop1 = ctx.next_hop("b");
        let hop2 = hop1.next_hop("c");
        let hop3 = hop2.next_hop("d");
        assert_eq!(hop3.hop_count(), 3);
        assert_eq!(hop3.correlation_id(), ctx.correlation_id());
    }

    #[test]
    fn header_round_trip() {
        let ctx = CorrelationContext::new("gateway", "txn-round-trip")
            .set_baggage("env", "prod")
            .set_baggage("region", "us-east-1");

        let headers = ctx.to_headers();
        let parsed = CorrelationContext::from_headers(&headers).unwrap();

        assert_eq!(parsed.correlation_id(), ctx.correlation_id());
        assert_eq!(parsed.origin_service(), ctx.origin_service());
        assert_eq!(parsed.hop_count(), ctx.hop_count());
        assert_eq!(parsed.baggage("env"), Some("prod"));
        assert_eq!(parsed.baggage("region"), Some("us-east-1"));
    }

    #[test]
    fn from_headers_missing_correlation_id() {
        let headers = alloc::vec![
            ("X-Origin-Service".to_string(), "svc".to_string()),
        ];
        assert_eq!(
            CorrelationContext::from_headers(&headers),
            Err(CorrelationError::MissingCorrelationId)
        );
    }

    #[test]
    fn from_headers_invalid_hop_count() {
        let ctx = CorrelationContext::new("svc", "seed");
        let mut headers = ctx.to_headers();
        for (k, v) in headers.iter_mut() {
            if k == HOP_COUNT_HEADER {
                *v = "not-a-number".to_string();
            }
        }
        assert!(matches!(
            CorrelationContext::from_headers(&headers),
            Err(CorrelationError::InvalidHopCount(_))
        ));
    }

    #[test]
    fn with_id_accepts_valid_id() {
        let valid = "abcdef0123456789abcdef0123456789";
        let ctx = CorrelationContext::with_id("svc", valid).unwrap();
        assert_eq!(ctx.correlation_id(), valid);
    }

    #[test]
    fn with_id_rejects_invalid_id() {
        let bad = "too-short";
        assert!(matches!(
            CorrelationContext::with_id("svc", bad),
            Err(CorrelationError::InvalidCorrelationId(_))
        ));
    }

    #[test]
    fn log_fields_format() {
        let ctx = CorrelationContext::new("gateway", "txn-log");
        let fields = ctx.log_fields();
        assert!(fields.contains("corr="));
        assert!(fields.contains("origin=gateway"));
        assert!(fields.contains("hop=0"));
    }
}
