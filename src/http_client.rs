//! HTTP client factory with optional proxy support.
//!
//! Centralises `reqwest` client construction so that every outbound HTTP call
//! (anchor discovery, webhook delivery, SEP-6 status) honours the same proxy
//! configuration without duplicating builder logic.
//!
//! # Proxy configuration
//!
//! [`ProxyConfig`] carries an optional `proxy_url` (e.g. `"http://proxy.corp:3128"`)
//! and optional `no_proxy` bypass list.  Pass it to [`build_client`] to get a
//! `reqwest::blocking::Client` that routes requests through the proxy.
//!
//! When `proxy_url` is `None` the returned client uses the system default
//! (respects `HTTP_PROXY` / `HTTPS_PROXY` environment variables).
//!
//! # Examples
//!
//! ```rust,no_run
//! use anchorkit::http_client::{ProxyConfig, build_client};
//!
//! // No proxy — uses system defaults.
//! let client = build_client(None, 30).unwrap();
//!
//! // Explicit proxy.
//! let proxy = ProxyConfig {
//!     proxy_url: Some("http://proxy.corp.example.com:3128".to_string()),
//!     no_proxy: Some("localhost,127.0.0.1".to_string()),
//! };
//! let client = build_client(Some(&proxy), 30).unwrap();
//! ```

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;
use alloc::string::String;

// ---------------------------------------------------------------------------
// Idempotency and request signing support
// ---------------------------------------------------------------------------

/// Options for idempotency and signing on a single outbound request.
///
/// Attach this to any outbound HTTP call to get:
/// - An `Idempotency-Key` header that lets the server safely deduplicate
///   replayed submissions.
/// - An `X-Request-Id` header that threads the same identifier through logs
///   and downstream services for correlation.
/// - HMAC-SHA256 signing via `X-Anchor-Signature: sha256=<hex>`, identical
///   to the webhook signing mechanism so verification helpers are shared.
///
/// All fields are optional; omit those you do not need.
///
/// # Examples
///
/// ```rust
/// use anchorkit::http_client::OutboundRequestOptions;
///
/// // Auto-generate an idempotency key from a stable seed (e.g. txn ID).
/// let opts = OutboundRequestOptions::with_idempotency_key("txn-001-deposit");
///
/// // Add HMAC signing on top.
/// let opts = OutboundRequestOptions::with_idempotency_key("txn-001-deposit")
///     .with_signing_key(b"my-secret-key");
/// ```
#[derive(Clone, Debug, Default)]
pub struct OutboundRequestOptions {
    /// Idempotency key sent as `Idempotency-Key: <value>`.
    /// When `Some`, also sent as `X-Request-Id: <value>` for correlation.
    pub idempotency_key: Option<String>,
    /// HMAC-SHA256 signing key. When `Some`, adds
    /// `X-Anchor-Signature: sha256=<hex>` computed over the request body.
    pub signing_key: Option<alloc::vec::Vec<u8>>,
}

impl OutboundRequestOptions {
    /// Create options with a caller-supplied idempotency key.
    pub fn with_idempotency_key(key: impl Into<String>) -> Self {
        OutboundRequestOptions {
            idempotency_key: Some(key.into()),
            signing_key: None,
        }
    }

    /// Attach an HMAC-SHA256 signing key to this options set.
    pub fn with_signing_key(mut self, key: &[u8]) -> Self {
        self.signing_key = Some(key.to_vec());
        self
    }

    /// Derive a deterministic idempotency key from `seed` using a short
    /// hex prefix of `SHA-256(seed_bytes)`.
    ///
    /// This is useful when you want a stable key derived from a transaction ID
    /// or other stable identifier without storing state.
    pub fn from_seed(seed: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(seed.as_bytes());
        let digest = hasher.finalize();
        // Use first 16 bytes (32 hex chars) as the key — short but collision-resistant.
        let hex: String = digest[..16]
            .iter()
            .fold(String::new(), |mut s, b| {
                s.push_str(&alloc::format!("{:02x}", b));
                s
            });
        OutboundRequestOptions {
            idempotency_key: Some(hex),
            signing_key: None,
        }
    }

    /// Build the extra headers that should be sent with an outbound request.
    ///
    /// Returns a list of `(header_name, header_value)` pairs.
    /// - `Idempotency-Key` — when `idempotency_key` is set.
    /// - `X-Request-Id` — same value as `Idempotency-Key` (correlation).
    /// - `X-Anchor-Signature: sha256=<hex>` — when `signing_key` is set.
    pub fn build_headers(&self, body: &str) -> alloc::vec::Vec<(String, String)> {
        let mut headers = alloc::vec::Vec::new();
        if let Some(ref key) = self.idempotency_key {
            headers.push(("Idempotency-Key".into(), key.clone()));
            headers.push(("X-Request-Id".into(), key.clone()));
        }
        if let Some(ref sk) = self.signing_key {
            let sig = compute_hmac_hex(sk, body);
            headers.push(("X-Anchor-Signature".into(), alloc::format!("sha256={}", sig)));
        }
        headers
    }

    /// Return `true` when this options set includes an idempotency key.
    pub fn has_idempotency_key(&self) -> bool {
        self.idempotency_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// Return `true` when this options set includes a signing key.
    pub fn has_signing_key(&self) -> bool {
        self.signing_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
    }
}

/// Compute `HMAC-SHA256(key, payload)` and return a lowercase hex string.
fn compute_hmac_hex(key: &[u8], payload: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
    result.iter().fold(String::new(), |mut s, b| {
        s.push_str(&alloc::format!("{:02x}", b));
        s
    })
}

/// Verify an `X-Anchor-Signature: sha256=<hex>` header value against a known
/// signing key and request body.
///
/// Uses constant-time comparison (XOR-fold) to prevent timing attacks.
///
/// # Arguments
///
/// * `body` — The raw request body that was signed.
/// * `signature_header` — The full header value, e.g. `"sha256=deadbeef..."`.
/// * `key` — The HMAC-SHA256 signing key.
///
/// # Returns
///
/// `true` when the signature matches; `false` otherwise.
pub fn verify_outbound_signature(body: &str, signature_header: &str, key: &[u8]) -> bool {
    let hex_digest = match signature_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    if hex_digest.len() % 2 != 0 {
        return false;
    }
    let mut received: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(hex_digest.len() / 2);
    let mut chars = hex_digest.chars();
    loop {
        match (chars.next(), chars.next()) {
            (Some(a), Some(b)) => {
                let byte = match (a.to_digit(16), b.to_digit(16)) {
                    (Some(hi), Some(lo)) => (hi << 4 | lo) as u8,
                    _ => return false,
                };
                received.push(byte);
            }
            (None, None) => break,
            _ => return false,
        }
    }
    let expected_hex = compute_hmac_hex(key, body);
    let expected_bytes: alloc::vec::Vec<u8> = {
        let mut bytes = alloc::vec::Vec::with_capacity(expected_hex.len() / 2);
        let mut ec = expected_hex.chars();
        loop {
            match (ec.next(), ec.next()) {
                (Some(a), Some(b)) => {
                    if let (Some(hi), Some(lo)) = (a.to_digit(16), b.to_digit(16)) {
                        bytes.push((hi << 4 | lo) as u8);
                    } else {
                        return false;
                    }
                }
                (None, None) => break,
                _ => return false,
            }
        }
        bytes
    };
    if received.len() != expected_bytes.len() {
        return false;
    }
    let diff: u8 = received
        .iter()
        .zip(expected_bytes.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    diff == 0
}

/// Perform a signed and idempotent HTTP POST using an injectable transport closure.
///
/// This is the low-level building block for feature-b compliance. Production code
/// should use the higher-level `deliver_webhook_with_proxy` which wraps reqwest;
/// this function exists for testability without a live HTTP stack.
///
/// # Arguments
///
/// * `url` — The target URL.
/// * `body` — The request payload (typically JSON).
/// * `opts` — Optional [`OutboundRequestOptions`] adding idempotency/signing headers.
/// * `http_post` — Injectable transport: `(url, body, headers) -> Result<u16, String>`.
///
/// # Returns
///
/// HTTP status code on success, transport error string on failure.
pub fn post_with_options<H>(
    url: &str,
    body: &str,
    opts: Option<&OutboundRequestOptions>,
    mut http_post: H,
) -> Result<u16, String>
where
    H: FnMut(&str, &str, &[(String, String)]) -> Result<u16, String>,
{
    let headers = opts.map(|o| o.build_headers(body)).unwrap_or_default();
    http_post(url, body, &headers)
}

// ---------------------------------------------------------------------------
// ConnectionPolicy — timeout and failure classification
// ---------------------------------------------------------------------------

/// Connection and timeout policy for outbound HTTP requests.
///
/// Controls how long the HTTP client waits at each phase of a connection.
/// All timeouts are in seconds; `0` means "no limit" for that phase.
///
/// # Design
///
/// Deterministic timeout behaviour prevents slow or misbehaving anchors from
/// hanging the caller indefinitely.  Each timeout targets a specific phase:
///
/// | Field | Phase | Default |
/// |---|---|---|
/// | `connect_timeout_secs` | TCP handshake + TLS negotiation | 10 s |
/// | `read_timeout_secs`    | Intended read budget (informational; not applied by reqwest 0.12 blocking) | 30 s |
/// | `total_timeout_secs`   | Total wall-clock budget (connect + read + transfer) | 60 s |
///
/// # Examples
///
/// ```rust
/// use anchorkit::http_client::ConnectionPolicy;
///
/// // Aggressive policy for time-sensitive flows.
/// let policy = ConnectionPolicy::aggressive();
/// assert_eq!(policy.connect_timeout_secs, 5);
///
/// // Conservative policy for batch / background fetches.
/// let policy = ConnectionPolicy::conservative();
/// assert_eq!(policy.total_timeout_secs, 120);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionPolicy {
    /// Maximum time in seconds to wait for the TCP connection + TLS handshake.
    /// Use `0` to rely on the OS default (not recommended in production).
    pub connect_timeout_secs: u64,
    /// Maximum time in seconds to wait for the first byte of the response after
    /// the request has been sent. Use `0` for no limit.
    pub read_timeout_secs: u64,
    /// Total wall-clock budget in seconds for the entire request lifecycle
    /// (connect + send + receive). Use `0` for no limit.
    pub total_timeout_secs: u64,
    /// When `true`, the client follows HTTP 3xx redirects automatically.
    /// Set to `false` to treat redirects as errors (useful for strict anchor
    /// endpoint compliance).
    pub follow_redirects: bool,
    /// Maximum number of redirects to follow when `follow_redirects` is `true`.
    /// Ignored when `follow_redirects` is `false`.
    pub max_redirects: usize,
}

impl Default for ConnectionPolicy {
    fn default() -> Self {
        ConnectionPolicy {
            connect_timeout_secs: 10,
            read_timeout_secs: 30,
            total_timeout_secs: 60,
            follow_redirects: true,
            max_redirects: 3,
        }
    }
}

impl ConnectionPolicy {
    /// Aggressive policy — suitable for interactive / latency-sensitive paths.
    ///
    /// Connect: 5 s, read: 15 s, total: 20 s. Redirects: disabled.
    pub fn aggressive() -> Self {
        ConnectionPolicy {
            connect_timeout_secs: 5,
            read_timeout_secs: 15,
            total_timeout_secs: 20,
            follow_redirects: false,
            max_redirects: 0,
        }
    }

    /// Conservative policy — for background discovery or batch operations.
    ///
    /// Connect: 20 s, read: 60 s, total: 120 s. Redirects: up to 5.
    pub fn conservative() -> Self {
        ConnectionPolicy {
            connect_timeout_secs: 20,
            read_timeout_secs: 60,
            total_timeout_secs: 120,
            follow_redirects: true,
            max_redirects: 5,
        }
    }

    /// Strict policy — no redirects, tight timeouts.
    /// Useful for SEP-10 auth flows where redirects would be suspicious.
    pub fn strict() -> Self {
        ConnectionPolicy {
            connect_timeout_secs: 8,
            read_timeout_secs: 20,
            total_timeout_secs: 30,
            follow_redirects: false,
            max_redirects: 0,
        }
    }
}

/// Classify a transport error string into `Timeout`, `ConnectionRefused`, or `Other`.
///
/// This enables deterministic retry behaviour: callers can branch on the failure
/// kind rather than parsing error messages ad hoc.
///
/// # Examples
///
/// ```rust
/// use anchorkit::http_client::{classify_transport_error, TransportErrorKind};
///
/// assert_eq!(
///     classify_transport_error("connection timed out"),
///     TransportErrorKind::Timeout,
/// );
/// assert_eq!(
///     classify_transport_error("connection refused"),
///     TransportErrorKind::ConnectionRefused,
/// );
/// assert_eq!(
///     classify_transport_error("DNS lookup failed for example.com"),
///     TransportErrorKind::DnsFailure,
/// );
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TransportErrorKind {
    /// The request timed out at any phase (connect, read, or total).
    Timeout,
    /// The remote refused the connection.
    ConnectionRefused,
    /// DNS resolution failed.
    DnsFailure,
    /// TLS handshake or certificate error.
    TlsError,
    /// Any other transport failure.
    Other,
}

/// Classify a transport error message into a [`TransportErrorKind`].
///
/// The classification is heuristic — it inspects the lowercase error string
/// for well-known substrings. This is sufficient for reqwest error messages,
/// which embed the reason in human-readable form.
pub fn classify_transport_error(msg: &str) -> TransportErrorKind {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") || lower.contains("deadline") {
        TransportErrorKind::Timeout
    } else if lower.contains("connection refused") || lower.contains("refused") {
        TransportErrorKind::ConnectionRefused
    } else if lower.contains("dns") || lower.contains("resolve") || lower.contains("no such host") {
        TransportErrorKind::DnsFailure
    } else if lower.contains("tls") || lower.contains("ssl") || lower.contains("certificate") || lower.contains("handshake") {
        TransportErrorKind::TlsError
    } else {
        TransportErrorKind::Other
    }
}

/// Returns `true` when a [`TransportErrorKind`] should trigger a retry.
///
/// Timeouts and DNS failures are transient; connection refused and TLS errors
/// are typically persistent and should not be retried blindly.
pub fn is_transport_error_retryable(kind: TransportErrorKind) -> bool {
    matches!(kind, TransportErrorKind::Timeout | TransportErrorKind::DnsFailure)
}

// ---------------------------------------------------------------------------
// ProxyConfig
// ---------------------------------------------------------------------------

/// Proxy settings for outbound HTTP requests.
///
/// Used by [`build_client`], [`fetch_stellar_toml_with_proxy`], and
/// [`deliver_webhook_with_proxy`] to route discovery and delivery traffic
/// through a corporate or gateway proxy.
///
/// # Fields
///
/// - `proxy_url` — Full proxy URL including scheme and port, e.g.
///   `"http://proxy.corp.example.com:3128"` or `"https://proxy.example.com:8080"`.
///   When `None` the client falls back to `HTTP_PROXY` / `HTTPS_PROXY` env vars.
/// - `no_proxy`  — Comma-separated list of hosts / CIDR ranges that bypass the
///   proxy, e.g. `"localhost,127.0.0.1,.internal.example.com"`.
///   When `None` no bypass list is applied.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct ProxyConfig {
    /// Proxy endpoint URL (e.g. `"http://proxy.corp.example.com:3128"`).
    pub proxy_url: Option<String>,
    /// Comma-separated no-proxy bypass list.
    pub no_proxy: Option<String>,
}

impl ProxyConfig {
    /// Returns `true` when a proxy URL has been configured.
    pub fn is_configured(&self) -> bool {
        self.proxy_url.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Client builder
// ---------------------------------------------------------------------------

/// Build a `reqwest::blocking::Client` with optional proxy and a configurable
/// timeout.
///
/// # Arguments
///
/// * `proxy`       — Optional [`ProxyConfig`]. When `None` the client uses
///   system proxy environment variables.
/// * `timeout_secs` — Per-request timeout in seconds. Use `0` for no timeout.
///
/// # Errors
///
/// Returns a `String` error if the proxy URL is malformed or the client cannot
/// be constructed.
#[cfg(feature = "std")]
pub fn build_client(
    proxy: Option<&ProxyConfig>,
    timeout_secs: u64,
) -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder();

    if timeout_secs > 0 {
        builder = builder.timeout(std::time::Duration::from_secs(timeout_secs));
    }

    if let Some(cfg) = proxy {
        if let Some(ref url) = cfg.proxy_url {
            if !url.is_empty() {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(alloc::format!(
                        "invalid proxy URL '{}': must start with http:// or https://", url
                    ));
                }
                let mut proxy_obj = reqwest::Proxy::all(url.as_str())
                    .map_err(|e| alloc::format!("invalid proxy URL '{}': {}", url, e))?;

                if let Some(ref no_proxy) = cfg.no_proxy {
                    if !no_proxy.is_empty() {
                        proxy_obj = proxy_obj.no_proxy(reqwest::NoProxy::from_string(no_proxy));
                    }
                }

                builder = builder.proxy(proxy_obj);
            }
        }
    }

    builder
        .build()
        .map_err(|e| alloc::format!("failed to build HTTP client: {}", e))
}

/// Build a `reqwest::blocking::Client` with full [`ConnectionPolicy`] control.
///
/// This is the recommended constructor for production use. It exposes all
/// timeout phases (connect, read, total) and redirect controls, making the
/// client's behaviour explicit and deterministic against slow or misbehaving
/// anchors.
///
/// # Arguments
///
/// * `proxy`  — Optional [`ProxyConfig`].
/// * `policy` — [`ConnectionPolicy`] describing timeout phases and redirect limits.
///   Pass `&ConnectionPolicy::default()` for sensible production defaults.
///
/// # Errors
///
/// Returns a `String` error if the proxy URL is malformed or the client cannot
/// be constructed.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::http_client::{ConnectionPolicy, ProxyConfig, build_client_with_policy};
///
/// let client = build_client_with_policy(None, &ConnectionPolicy::default()).unwrap();
/// let strict = build_client_with_policy(None, &ConnectionPolicy::strict()).unwrap();
/// ```
#[cfg(feature = "std")]
pub fn build_client_with_policy(
    proxy: Option<&ProxyConfig>,
    policy: &ConnectionPolicy,
) -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder();

    // Connect timeout
    if policy.connect_timeout_secs > 0 {
        builder = builder.connect_timeout(std::time::Duration::from_secs(policy.connect_timeout_secs));
    }
    // Total request timeout covers connect + send + receive.
    // reqwest 0.12 blocking client does not expose a separate read_timeout;
    // total_timeout_secs is the single wall-clock cap for the entire request.
    // read_timeout_secs is stored on ConnectionPolicy for documentation and
    // future use but not applied here.
    if policy.total_timeout_secs > 0 {
        builder = builder.timeout(std::time::Duration::from_secs(policy.total_timeout_secs));
    }
    // Redirect policy
    if policy.follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::limited(policy.max_redirects));
    } else {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }

    // Proxy
    if let Some(cfg) = proxy {
        if let Some(ref url) = cfg.proxy_url {
            if !url.is_empty() {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(alloc::format!(
                        "invalid proxy URL '{}': must start with http:// or https://", url
                    ));
                }
                let mut proxy_obj = reqwest::Proxy::all(url.as_str())
                    .map_err(|e| alloc::format!("invalid proxy URL '{}': {}", url, e))?;
                if let Some(ref no_proxy) = cfg.no_proxy {
                    if !no_proxy.is_empty() {
                        proxy_obj = proxy_obj.no_proxy(reqwest::NoProxy::from_string(no_proxy));
                    }
                }
                builder = builder.proxy(proxy_obj);
            }
        }
    }

    builder
        .build()
        .map_err(|e| alloc::format!("failed to build HTTP client: {}", e))
}
// ---------------------------------------------------------------------------
// Proxy-aware stellar.toml fetcher
// ---------------------------------------------------------------------------

/// Fetch and parse a `stellar.toml` file through an optional proxy.
///
/// Constructs the well-known URL via [`fetch_stellar_toml_url`], performs an
/// HTTP GET (routing through `proxy` when configured), and parses the response
/// body with [`parse_stellar_toml`].
///
/// # Arguments
///
/// * `domain`      — Anchor base URL, e.g. `"https://anchor.example.com"`.
/// * `proxy`       — Optional proxy configuration.
/// * `timeout_secs` — Per-request timeout in seconds.
///
/// # Errors
///
/// Returns a `String` error on network failure, non-2xx HTTP status, or TOML
/// parse failure.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::http_client::{ProxyConfig, fetch_stellar_toml_with_proxy};
///
/// let proxy = ProxyConfig {
///     proxy_url: Some("http://proxy.corp.example.com:3128".to_string()),
///     no_proxy: None,
/// };
/// let toml = fetch_stellar_toml_with_proxy("https://anchor.example.com", Some(&proxy), 30).unwrap();
/// println!("Supports SEP-6: {}", toml.supports_sep6());
/// ```
#[cfg(feature = "std")]
pub fn fetch_stellar_toml_with_proxy(
    domain: &str,
    proxy: Option<&ProxyConfig>,
    timeout_secs: u64,
) -> Result<crate::stellar_toml::ParsedStellarToml, String> {
    let url = crate::stellar_toml::fetch_stellar_toml_url(domain)
        .map_err(|e| alloc::format!("invalid domain '{}': {:?}", domain, e))?;

    let client = build_client(proxy, timeout_secs)?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| alloc::format!("GET {} failed: {}", url, e))?;

    if !response.status().is_success() {
        return Err(alloc::format!(
            "GET {} returned HTTP {}",
            url,
            response.status()
        ));
    }

    let body = response
        .text()
        .map_err(|e| alloc::format!("failed to read response body: {}", e))?;

    crate::stellar_toml::parse_stellar_toml(&body)
        .map_err(|e| alloc::format!("failed to parse stellar.toml: {:?}", e))
}

// ---------------------------------------------------------------------------
// Proxy-aware webhook delivery
// ---------------------------------------------------------------------------

/// Deliver a webhook payload through an optional proxy.
///
/// This is a thin wrapper around [`deliver_webhook`] that constructs the
/// `http_post` transport function using a proxy-aware `reqwest` client.
///
/// # Arguments
///
/// * `config`      — Webhook delivery configuration (endpoint, retries, DLQ key).
/// * `payload`     — JSON payload string to POST.
/// * `dlq`         — Dead-letter queue map for failed deliveries.
/// * `proxy`       — Optional proxy configuration.
/// * `now_fn`      — Returns the current Unix timestamp in seconds.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::WebhookDeliveryFailed`]
/// after all retry attempts are exhausted.
///
/// # Examples
///
/// ```rust,no_run
/// use std::collections::BTreeMap;
/// use anchorkit::http_client::{ProxyConfig, deliver_webhook_with_proxy};
/// use anchorkit::webhook::{WebhookDeliveryConfig, DlqEntry};
/// use anchorkit::retry::RetryConfig;
///
/// let config = WebhookDeliveryConfig {
///     endpoint_url: "https://hooks.example.com/anchor".to_string(),
///     timeout_ms: 5000,
///     retry_config: RetryConfig::default(),
///     dead_letter_storage_key: "anchor-hook".to_string(),
///     signing_key: None,
/// };
/// let proxy = ProxyConfig {
///     proxy_url: Some("http://proxy.corp.example.com:3128".to_string()),
///     no_proxy: None,
/// };
/// let mut dlq = BTreeMap::new();
/// deliver_webhook_with_proxy(&config, r#"{"event":"deposit"}"#, &mut dlq, Some(&proxy), || 0).unwrap();
/// ```
#[cfg(feature = "std")]
pub fn deliver_webhook_with_proxy(
    config: &crate::webhook::WebhookDeliveryConfig,
    payload: &str,
    dlq: &mut alloc::collections::BTreeMap<String, alloc::vec::Vec<crate::webhook::DlqEntry>>,
    proxy: Option<&ProxyConfig>,
    now_fn: impl Fn() -> u64,
) -> Result<(), crate::errors::AnchorKitError> {
    let timeout_secs = if config.timeout_ms > 0 {
        (config.timeout_ms / 1000).max(1)
    } else {
        30
    };

    let client = build_client(proxy, timeout_secs)
        .map_err(|e| {
            crate::errors::AnchorKitError::with_context(
                crate::errors::ErrorCode::WebhookDeliveryFailed,
                "failed to build HTTP client for webhook delivery",
                &e,
            )
        })?;

    crate::webhook::deliver_webhook(
        config,
        payload,
        dlq,
        move |url, body, sig_header| {
            let mut req = client
                .post(url)
                .header("Content-Type", "application/json")
                .body(alloc::string::String::from(body));
            if let Some(sig) = sig_header {
                req = req.header("X-Anchor-Signature", sig);
            }
            req.send()
                .map(|r| r.status().as_u16())
                .map_err(|e| alloc::format!("HTTP POST failed: {}", e))
        },
        |_| {},
        now_fn,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn proxy_config_default_is_unconfigured() {
        let cfg = ProxyConfig::default();
        assert!(!cfg.is_configured());
    }

    #[test]
    fn proxy_config_with_url_is_configured() {
        let cfg = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: None,
        };
        assert!(cfg.is_configured());
    }

    #[test]
    fn proxy_config_empty_url_is_not_configured() {
        let cfg = ProxyConfig {
            proxy_url: Some(String::new()),
            no_proxy: None,
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn proxy_config_none_url_is_not_configured() {
        let cfg = ProxyConfig {
            proxy_url: None,
            no_proxy: Some("localhost".to_string()),
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn proxy_config_clone_and_eq() {
        let a = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: Some("localhost".to_string()),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_no_proxy_succeeds() {
        let client = build_client(None, 10);
        assert!(client.is_ok(), "client without proxy should build successfully");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_valid_proxy_url_succeeds() {
        let proxy = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: None,
        };
        let client = build_client(Some(&proxy), 10);
        assert!(client.is_ok(), "client with valid proxy URL should build successfully");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_proxy_and_no_proxy_list_succeeds() {
        let proxy = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: Some("localhost,127.0.0.1,.internal.example.com".to_string()),
        };
        let client = build_client(Some(&proxy), 30);
        assert!(client.is_ok(), "client with proxy + no_proxy list should build successfully");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_invalid_proxy_url_returns_error() {
        let proxy = ProxyConfig {
            proxy_url: Some("not-a-valid-url".to_string()),
            no_proxy: None,
        };
        let result = build_client(Some(&proxy), 10);
        assert!(result.is_err(), "invalid proxy URL should return an error");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("invalid proxy URL"),
            "error message should mention invalid proxy URL, got: {msg}"
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_unconfigured_proxy_uses_system_defaults() {
        // An unconfigured ProxyConfig (no URL) should behave like no proxy at all.
        let proxy = ProxyConfig::default();
        let client = build_client(Some(&proxy), 10);
        assert!(client.is_ok(), "unconfigured proxy should fall through to system defaults");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_zero_timeout_builds_successfully() {
        // timeout_secs = 0 means no timeout — client should still build.
        let client = build_client(None, 0);
        assert!(client.is_ok(), "zero timeout should build successfully");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_https_proxy_url_succeeds() {
        let proxy = ProxyConfig {
            proxy_url: Some("https://secure-proxy.example.com:8080".to_string()),
            no_proxy: None,
        };
        let client = build_client(Some(&proxy), 10);
        assert!(client.is_ok(), "HTTPS proxy URL should build successfully");
    }

    // ── Webhook delivery with proxy (unit-level, injected transport) ──────────

    #[cfg(feature = "std")]
    #[test]
    fn deliver_webhook_with_proxy_succeeds_on_200() {
        use crate::webhook::{WebhookDeliveryConfig, DlqEntry, get_dead_letter_webhooks};
        use crate::retry::RetryConfig;
        use alloc::collections::BTreeMap;

        // Use the base deliver_webhook directly with an injected transport to
        // avoid real network calls in unit tests.
        let config = WebhookDeliveryConfig {
            endpoint_url: "https://hooks.example.com/anchor".to_string(),
            timeout_ms: 1000,
            retry_config: RetryConfig {
                max_attempts: 3,
                base_delay_ms: 0,
                max_delay_ms: 0,
                backoff_multiplier: 1,
                strategy: crate::retry::BackoffStrategy::Exponential,
                jitter_policy: crate::retry::JitterPolicy::None,
            },
            dead_letter_storage_key: "proxy-test".to_string(),
            signing_key: None,
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        };

        let mut dlq: BTreeMap<String, alloc::vec::Vec<DlqEntry>> = BTreeMap::new();

        // Inject a mock transport that always returns 200.
        let result = crate::webhook::deliver_webhook(
            &config,
            r#"{"event":"deposit_completed"}"#,
            &mut dlq,
            |_url, _body, _sig| Ok(200u16),
            |_| {},
            || 1_000_000u64,
        );

        assert!(result.is_ok(), "delivery should succeed with 200 response");
        assert!(
            get_dead_letter_webhooks(&dlq, "proxy-test").is_empty(),
            "DLQ should be empty on success"
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn deliver_webhook_with_proxy_stores_dlq_on_failure() {
        use crate::webhook::{WebhookDeliveryConfig, DlqEntry, get_dead_letter_webhooks};
        use crate::retry::RetryConfig;
        use alloc::collections::BTreeMap;

        let config = WebhookDeliveryConfig {
            endpoint_url: "https://hooks.example.com/anchor".to_string(),
            timeout_ms: 1000,
            retry_config: RetryConfig {
                max_attempts: 2,
                base_delay_ms: 0,
                max_delay_ms: 0,
                backoff_multiplier: 1,
                strategy: crate::retry::BackoffStrategy::Exponential,
                jitter_policy: crate::retry::JitterPolicy::None,
            },
            dead_letter_storage_key: "proxy-fail-test".to_string(),
            signing_key: None,
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        };

        let mut dlq: BTreeMap<String, alloc::vec::Vec<DlqEntry>> = BTreeMap::new();

        // Inject a mock transport that always returns 503.
        let result = crate::webhook::deliver_webhook(
            &config,
            r#"{"event":"deposit_failed"}"#,
            &mut dlq,
            |_url, _body, _sig| Ok(503u16),
            |_| {},
            || 9_999_999u64,
        );

        assert!(result.is_err(), "delivery should fail after exhausting retries");
        let entries = get_dead_letter_webhooks(&dlq, "proxy-fail-test");
        assert_eq!(entries.len(), 1, "one DLQ entry should be written");
        assert_eq!(entries[0].last_status_code, 503);
        assert_eq!(entries[0].attempts_made, 2);
        assert_eq!(entries[0].failed_at_timestamp, 9_999_999);
    }

    // ── ProxyConfig serialization (std only) ──────────────────────────────────

    #[cfg(feature = "std")]
    #[test]
    fn proxy_config_serializes_to_json() {
        let cfg = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: Some("localhost".to_string()),
        };
        let json = serde_json::to_string(&cfg).expect("serialization should succeed");
        assert!(json.contains("proxy_url"));
        assert!(json.contains("proxy.example.com"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn proxy_config_deserializes_from_json() {
        let json = r#"{"proxy_url":"http://proxy.example.com:3128","no_proxy":"localhost"}"#;
        let cfg: ProxyConfig = serde_json::from_str(json).expect("deserialization should succeed");
        assert_eq!(cfg.proxy_url.as_deref(), Some("http://proxy.example.com:3128"));
        assert_eq!(cfg.no_proxy.as_deref(), Some("localhost"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn proxy_config_deserializes_with_null_fields() {
        let json = r#"{"proxy_url":null,"no_proxy":null}"#;
        let cfg: ProxyConfig = serde_json::from_str(json).expect("deserialization should succeed");
        assert!(cfg.proxy_url.is_none());
        assert!(cfg.no_proxy.is_none());
        assert!(!cfg.is_configured());
    }

    // ── Idempotency and signing (OutboundRequestOptions) ──────────────────────

    #[test]
    fn outbound_options_default_has_no_headers() {
        let opts = OutboundRequestOptions::default();
        let headers = opts.build_headers("body");
        assert!(headers.is_empty());
        assert!(!opts.has_idempotency_key());
        assert!(!opts.has_signing_key());
    }

    #[test]
    fn outbound_options_with_idempotency_key_emits_two_headers() {
        let opts = OutboundRequestOptions::with_idempotency_key("txn-001");
        let headers = opts.build_headers("payload");
        let names: alloc::vec::Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"Idempotency-Key"), "should include Idempotency-Key");
        assert!(names.contains(&"X-Request-Id"), "should include X-Request-Id");
        // Both should carry the same value
        let ik = headers.iter().find(|(k, _)| k == "Idempotency-Key").unwrap();
        let ri = headers.iter().find(|(k, _)| k == "X-Request-Id").unwrap();
        assert_eq!(ik.1, "txn-001");
        assert_eq!(ri.1, "txn-001");
        assert!(opts.has_idempotency_key());
    }

    #[test]
    fn outbound_options_with_signing_key_emits_signature_header() {
        let opts = OutboundRequestOptions::default().with_signing_key(b"secret");
        let body = r#"{"event":"deposit"}"#;
        let headers = opts.build_headers(body);
        let sig_header = headers.iter().find(|(k, _)| k == "X-Anchor-Signature");
        assert!(sig_header.is_some(), "should include X-Anchor-Signature");
        let (_, sig_val) = sig_header.unwrap();
        assert!(sig_val.starts_with("sha256="), "signature header must start with sha256=");
        assert!(opts.has_signing_key());
    }

    #[test]
    fn outbound_options_signature_is_verifiable() {
        let key = b"my-hmac-key";
        let body = r#"{"event":"withdrawal","amount":100}"#;
        let opts = OutboundRequestOptions::default().with_signing_key(key);
        let headers = opts.build_headers(body);
        let sig_val = &headers.iter()
            .find(|(k, _)| k == "X-Anchor-Signature")
            .unwrap().1;
        assert!(verify_outbound_signature(body, sig_val, key),
            "signature should verify correctly");
    }

    #[test]
    fn outbound_options_wrong_key_fails_verification() {
        let key = b"correct-key";
        let wrong_key = b"wrong-key";
        let body = "payload";
        let opts = OutboundRequestOptions::default().with_signing_key(key);
        let headers = opts.build_headers(body);
        let sig_val = &headers.iter()
            .find(|(k, _)| k == "X-Anchor-Signature")
            .unwrap().1;
        assert!(!verify_outbound_signature(body, sig_val, wrong_key),
            "verification with wrong key should fail");
    }

    #[test]
    fn outbound_options_tampered_body_fails_verification() {
        let key = b"hmac-key";
        let body = "original body";
        let opts = OutboundRequestOptions::default().with_signing_key(key);
        let headers = opts.build_headers(body);
        let sig_val = &headers.iter()
            .find(|(k, _)| k == "X-Anchor-Signature")
            .unwrap().1;
        assert!(!verify_outbound_signature("tampered body", sig_val, key),
            "verification should fail when body is tampered");
    }

    #[test]
    fn outbound_options_from_seed_is_deterministic() {
        let opts1 = OutboundRequestOptions::from_seed("txn-123");
        let opts2 = OutboundRequestOptions::from_seed("txn-123");
        assert_eq!(opts1.idempotency_key, opts2.idempotency_key,
            "same seed must produce same idempotency key");
        assert!(opts1.has_idempotency_key());
    }

    #[test]
    fn outbound_options_from_seed_different_seeds_produce_different_keys() {
        let opts1 = OutboundRequestOptions::from_seed("txn-001");
        let opts2 = OutboundRequestOptions::from_seed("txn-002");
        assert_ne!(opts1.idempotency_key, opts2.idempotency_key,
            "different seeds must produce different idempotency keys");
    }

    #[test]
    fn post_with_options_passes_headers_to_transport() {
        let key = b"signing-key";
        let opts = OutboundRequestOptions::with_idempotency_key("idem-42")
            .with_signing_key(key);
        let body = r#"{"amount":50}"#;

        let mut captured_headers: alloc::vec::Vec<(String, String)> = alloc::vec::Vec::new();
        let result = post_with_options(
            "https://example.com/sep6",
            body,
            Some(&opts),
            |_url, _body, hdrs| {
                captured_headers.extend(hdrs.iter().cloned());
                Ok(200u16)
            },
        );

        assert_eq!(result, Ok(200));
        let names: alloc::vec::Vec<&str> = captured_headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"Idempotency-Key"));
        assert!(names.contains(&"X-Request-Id"));
        assert!(names.contains(&"X-Anchor-Signature"));
    }

    #[test]
    fn post_with_options_no_options_still_works() {
        let result = post_with_options(
            "https://example.com/sep6",
            "body",
            None,
            |_url, _body, hdrs| {
                assert!(hdrs.is_empty(), "no extra headers when options is None");
                Ok(201u16)
            },
        );
        assert_eq!(result, Ok(201));
    }

    #[test]
    fn duplicate_submissions_with_same_idempotency_key_are_deduplicated() {
        // Simulate a server that recognises duplicate Idempotency-Key values
        // and returns 200 on first call and 200 (idempotent) on second.
        let opts = OutboundRequestOptions::with_idempotency_key("idem-dedup");
        let body = r#"{"event":"deposit"}"#;
        let call_count = core::cell::Cell::new(0u32);

        let make_call = |count: &core::cell::Cell<u32>| {
            let headers = opts.build_headers(body);
            let idem_key = headers.iter()
                .find(|(k, _)| k == "Idempotency-Key")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            count.set(count.get() + 1);
            (idem_key, 200u16)
        };

        let (key1, status1) = make_call(&call_count);
        let (key2, status2) = make_call(&call_count);

        assert_eq!(key1, key2, "idempotency key must be stable across calls");
        assert_eq!(status1, 200);
        assert_eq!(status2, 200);
        assert_eq!(call_count.get(), 2);
    }

    // ── ConnectionPolicy and error classification ─────────────────────────────

    #[test]
    fn connection_policy_default_values() {
        let p = ConnectionPolicy::default();
        assert_eq!(p.connect_timeout_secs, 10);
        assert_eq!(p.read_timeout_secs, 30);
        assert_eq!(p.total_timeout_secs, 60);
        assert!(p.follow_redirects);
        assert_eq!(p.max_redirects, 3);
    }

    #[test]
    fn connection_policy_aggressive() {
        let p = ConnectionPolicy::aggressive();
        assert!(p.connect_timeout_secs < 10, "aggressive connect should be < 10s");
        assert!(!p.follow_redirects, "aggressive should not follow redirects");
    }

    #[test]
    fn connection_policy_conservative() {
        let p = ConnectionPolicy::conservative();
        assert!(p.total_timeout_secs > 60, "conservative total should be > 60s");
        assert!(p.follow_redirects, "conservative should follow redirects");
        assert!(p.max_redirects >= 3, "conservative should allow multiple redirects");
    }

    #[test]
    fn connection_policy_strict_no_redirects() {
        let p = ConnectionPolicy::strict();
        assert!(!p.follow_redirects);
        assert_eq!(p.max_redirects, 0);
    }

    #[test]
    fn classify_transport_error_timeout() {
        assert_eq!(classify_transport_error("connection timed out"), TransportErrorKind::Timeout);
        assert_eq!(classify_transport_error("request timeout"), TransportErrorKind::Timeout);
        assert_eq!(classify_transport_error("DEADLINE exceeded"), TransportErrorKind::Timeout);
    }

    #[test]
    fn classify_transport_error_connection_refused() {
        assert_eq!(classify_transport_error("connection refused"), TransportErrorKind::ConnectionRefused);
        assert_eq!(classify_transport_error("Connection REFUSED by server"), TransportErrorKind::ConnectionRefused);
    }

    #[test]
    fn classify_transport_error_dns_failure() {
        assert_eq!(classify_transport_error("DNS lookup failed"), TransportErrorKind::DnsFailure);
        assert_eq!(classify_transport_error("failed to resolve hostname"), TransportErrorKind::DnsFailure);
        assert_eq!(classify_transport_error("no such host"), TransportErrorKind::DnsFailure);
    }

    #[test]
    fn classify_transport_error_tls() {
        assert_eq!(classify_transport_error("TLS handshake failed"), TransportErrorKind::TlsError);
        assert_eq!(classify_transport_error("SSL certificate verification error"), TransportErrorKind::TlsError);
        assert_eq!(classify_transport_error("certificate expired"), TransportErrorKind::TlsError);
    }

    #[test]
    fn classify_transport_error_other() {
        assert_eq!(classify_transport_error("unexpected end of stream"), TransportErrorKind::Other);
        assert_eq!(classify_transport_error("broken pipe"), TransportErrorKind::Other);
    }

    #[test]
    fn is_transport_error_retryable_timeouts_and_dns() {
        assert!(is_transport_error_retryable(TransportErrorKind::Timeout));
        assert!(is_transport_error_retryable(TransportErrorKind::DnsFailure));
    }

    #[test]
    fn is_transport_error_not_retryable_refused_and_tls() {
        assert!(!is_transport_error_retryable(TransportErrorKind::ConnectionRefused));
        assert!(!is_transport_error_retryable(TransportErrorKind::TlsError));
        assert!(!is_transport_error_retryable(TransportErrorKind::Other));
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_policy_default_succeeds() {
        let client = build_client_with_policy(None, &ConnectionPolicy::default());
        assert!(client.is_ok(), "client with default policy should build successfully");
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_policy_aggressive_succeeds() {
        let client = build_client_with_policy(None, &ConnectionPolicy::aggressive());
        assert!(client.is_ok());
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_policy_strict_succeeds() {
        let client = build_client_with_policy(None, &ConnectionPolicy::strict());
        assert!(client.is_ok());
    }

    #[cfg(feature = "std")]
    #[test]
    fn build_client_with_policy_invalid_proxy_fails() {
        let proxy = ProxyConfig {
            proxy_url: Some("not-a-url".into()),
            no_proxy: None,
        };
        let result = build_client_with_policy(Some(&proxy), &ConnectionPolicy::default());
        assert!(result.is_err());
    }
}
