//! Webhook delivery with exponential backoff and a Dead Letter Queue (DLQ).
//!
//! `deliver_webhook` wraps the HTTP POST in `retry_with_backoff`.  On total
//! exhaustion a structured [`DlqEntry`] is written into the caller-supplied DLQ
//! map under `dead_letter_storage_key`.  `get_dead_letter_webhooks` and
//! `query_dlq` let admins inspect those failed entries.

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
};

use alloc::vec::Vec;
use core::cell::RefCell;

use crate::{
    errors::{AnchorKitError, ErrorCode},
    retry::{retry_with_backoff_traced, RetryConfig},
    trace_context::TraceContext,
};

// ---------------------------------------------------------------------------
// HMAC-SHA256 signing helpers
// ---------------------------------------------------------------------------

/// Compute HMAC-SHA256(`key`, `payload`) and return a lowercase hex string.
fn sign_payload(key: &[u8], payload: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
    result.iter().fold(String::new(), |mut s, b| {
        use alloc::format;
        s.push_str(&format!("{:02x}", b));
        s
    })
}

/// Verify that `signature_header` (format `sha256=<hex>`) matches
/// HMAC-SHA256(`key`, `payload`).
///
/// The comparison is done byte-by-byte in constant time to prevent timing
/// attacks.
pub fn verify_webhook_signature(payload: &str, signature_header: &str, key: &[u8]) -> bool {
    let hex_digest = match signature_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    // Hex-decode the received digest.
    if hex_digest.len() % 2 != 0 {
        return false;
    }
    let mut received = Vec::with_capacity(hex_digest.len() / 2);
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
    // Compute expected digest.
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    // Constant-time comparison via XOR.
    let expected = mac.finalize().into_bytes();
    if received.len() != expected.len() {
        return false;
    }
    let diff: u8 = received.iter().zip(expected.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b));
    diff == 0
}

/// Result of webhook signature verification with replay protection.
#[derive(Debug, PartialEq)]
pub enum VerificationResult {
    /// Signature is valid and message is fresh
    Valid,
    /// Signature is invalid (malformed or doesn't match)
    InvalidSignature,
    /// Message timestamp is too old or in the future
    InvalidTimestamp,
    /// Nonce has already been used (replay attack)
    ReplayDetected,
}

/// Verify webhook signature with replay protection.
///
/// This function provides enhanced security with:
/// 1. HMAC-SHA256 signature verification (constant-time)
/// 2. Timestamp freshness check (within `max_age_seconds`)
/// 3. Nonce tracking to prevent replay attacks
///
/// The payload should contain a JSON object with at least:
/// - `timestamp`: Unix timestamp (seconds)
/// - `nonce`: Unique string for replay protection
pub fn verify_webhook_signature_with_replay_protection(
    payload: &str,
    signature_header: &str,
    key: &[u8],
    current_time: u64,
    max_age_seconds: u64,
    nonce_tracker: &mut impl NonceTracker,
) -> VerificationResult {
    // First, verify the signature
    if !verify_webhook_signature(payload, signature_header, key) {
        return VerificationResult::InvalidSignature;
    }
    
    // Parse the payload to extract timestamp and nonce
    let (timestamp, nonce) = match extract_timestamp_and_nonce(payload) {
        Ok((ts, n)) => (ts, n),
        Err(_) => return VerificationResult::InvalidTimestamp,
    };
    
    // Check timestamp freshness
    if timestamp > current_time {
        // Timestamp in the future - reject
        return VerificationResult::InvalidTimestamp;
    }
    
    let age = current_time.saturating_sub(timestamp);
    if age > max_age_seconds {
        return VerificationResult::InvalidTimestamp;
    }
    
    // Check for replay attacks using nonce
    if !nonce_tracker.check_and_record(&nonce, timestamp) {
        return VerificationResult::ReplayDetected;
    }
    
    VerificationResult::Valid
}

/// Extract timestamp and nonce from a JSON payload.
///
/// Expected payload format: JSON object with `timestamp` and `nonce` fields.
fn extract_timestamp_and_nonce(payload: &str) -> Result<(u64, String), ()> {
    // Simple JSON parsing for timestamp and nonce
    // This is a simplified implementation - in production you might want to use a proper JSON parser
    
    // Look for timestamp field
    let timestamp_start = payload.find("\"timestamp\":");
    let nonce_start = payload.find("\"nonce\":");
    
    if timestamp_start.is_none() || nonce_start.is_none() {
        return Err(());
    }
    
    // Extract timestamp value
    let ts_str = &payload[timestamp_start.unwrap() + 12..];
    let ts_end = ts_str.find(|c: char| !c.is_ascii_digit()).unwrap_or(ts_str.len());
    let timestamp_str = &ts_str[..ts_end];
    let timestamp: u64 = timestamp_str.parse().map_err(|_| ())?;
    
    // Extract nonce value (simplified - assumes nonce is in quotes)
    let nonce_str = &payload[nonce_start.unwrap() + 8..];
    let quote_start = nonce_str.find('"').ok_or(())?;
    let after_quote = &nonce_str[quote_start + 1..];
    let quote_end = after_quote.find('"').ok_or(())?;
    let nonce = after_quote[..quote_end].to_string();
    
    Ok((timestamp, nonce))
}

/// Trait for tracking nonces to prevent replay attacks.
pub trait NonceTracker {
    /// Check if a nonce has been used before and record it if not.
    /// Returns `true` if the nonce is new and should be accepted.
    /// Returns `false` if the nonce has already been used (replay attack).
    fn check_and_record(&mut self, nonce: &str, timestamp: u64) -> bool;
    
    /// Cleanup expired nonces (older than `max_age_seconds`).
    fn cleanup_expired(&mut self, current_time: u64, max_age_seconds: u64);
}

/// Simple in-memory nonce tracker for webhook replay protection.
pub struct MemoryNonceTracker {
    nonces: alloc::collections::BTreeMap<String, u64>,
    max_capacity: usize,
}

impl MemoryNonceTracker {
    /// Create a new nonce tracker with unlimited capacity.
    pub fn new() -> Self {
        MemoryNonceTracker {
            nonces: alloc::collections::BTreeMap::new(),
            max_capacity: usize::MAX,
        }
    }
    
    /// Create a new nonce tracker with capacity limit.
    pub fn with_capacity(max_capacity: usize) -> Self {
        MemoryNonceTracker {
            nonces: alloc::collections::BTreeMap::new(),
            max_capacity,
        }
    }
}

impl NonceTracker for MemoryNonceTracker {
    fn check_and_record(&mut self, nonce: &str, timestamp: u64) -> bool {
        // Check if nonce already exists
        if self.nonces.contains_key(nonce) {
            return false;
        }
        
        // Check capacity and remove oldest if needed
        if self.nonces.len() >= self.max_capacity {
            // Find and remove the oldest nonce
            if let Some(oldest_key) = self.nonces
                .iter()
                .min_by_key(|(_, &ts)| ts)
                .map(|(k, _)| k.clone())
            {
                self.nonces.remove(&oldest_key);
            }
        }
        
        // Record the new nonce
        self.nonces.insert(nonce.to_string(), timestamp);
        true
    }
    
    fn cleanup_expired(&mut self, current_time: u64, max_age_seconds: u64) {
        let cutoff = current_time.saturating_sub(max_age_seconds);
        self.nonces.retain(|_, ts| *ts >= cutoff);
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a single webhook endpoint.
///
/// Retry behaviour is fully described by [`RetryConfig`]: `retry_config.max_attempts`
/// is the single source of truth for how many delivery attempts are made, and
/// `retry_config.base_delay_ms` (with the multiplier/cap) controls the backoff
/// delay. There are intentionally no separate `max_retries` / `retry_delay_ms`
/// fields — they previously duplicated `RetryConfig` and could silently disagree.
#[derive(Clone, Debug)]
pub struct WebhookDeliveryConfig {
    /// Target URL for the HTTP POST.
    pub endpoint_url: String,
    /// Per-attempt timeout in milliseconds (informational; enforced by `http_post`).
    pub timeout_ms: u64,
    /// Backoff parameters: max attempts, base delay, multiplier, cap.
    pub retry_config: RetryConfig,
    /// Key under which failed entries are stored in the DLQ map.
    pub dead_letter_storage_key: String,
    /// Optional HMAC-SHA256 signing key. When `Some`, an `X-Anchor-Signature`
    /// header of the form `sha256=<hex>` is appended to every HTTP POST.
    /// Existing configs that omit this field continue to work unsigned.
    pub signing_key: Option<Vec<u8>>,
    /// Maximum allowed age of signed webhook payloads in seconds.
    /// When `Some`, the payload must contain a `timestamp` field and the
    /// signature verification will reject messages older than this threshold.
    /// Default: 300 seconds (5 minutes).
    pub max_payload_age_seconds: Option<u64>,
    /// Whether to require a `nonce` field in signed webhook payloads for
    /// replay protection. When `true`, each nonce can only be used once.
    pub require_nonce_for_replay_protection: bool,
}

// ---------------------------------------------------------------------------
// Queue abstraction
// ---------------------------------------------------------------------------

/// A FIFO queue for webhook delivery with configurable capacity and retention.
pub struct WebhookQueue<T> {
    entries: alloc::collections::VecDeque<T>,
    max_capacity: Option<usize>,
    retention_seconds: Option<u64>,
}

impl<T> WebhookQueue<T> {
    /// Create a new queue with no capacity or retention limits.
    pub fn new() -> Self {
        WebhookQueue {
            entries: alloc::collections::VecDeque::new(),
            max_capacity: None,
            retention_seconds: None,
        }
    }
    
    /// Create a new queue with a maximum capacity.
    /// When full, oldest entries are removed to make space for new ones.
    pub fn with_capacity(max_capacity: usize) -> Self {
        WebhookQueue {
            entries: alloc::collections::VecDeque::new(),
            max_capacity: Some(max_capacity),
            retention_seconds: None,
        }
    }
    
    /// Create a new queue with retention policy.
    /// Entries older than `retention_seconds` will be automatically removed
    /// during cleanup operations.
    pub fn with_retention(retention_seconds: u64) -> Self {
        WebhookQueue {
            entries: alloc::collections::VecDeque::new(),
            max_capacity: None,
            retention_seconds: Some(retention_seconds),
        }
    }
    
    /// Create a new queue with both capacity and retention limits.
    pub fn with_capacity_and_retention(max_capacity: usize, retention_seconds: u64) -> Self {
        WebhookQueue {
            entries: alloc::collections::VecDeque::new(),
            max_capacity: Some(max_capacity),
            retention_seconds: Some(retention_seconds),
        }
    }
    
    /// Add an entry to the back of the queue.
    /// If the queue is at capacity, removes the oldest entry first.
    pub fn enqueue(&mut self, entry: T) {
        if let Some(capacity) = self.max_capacity {
            if self.entries.len() >= capacity {
                self.entries.pop_front(); // Remove oldest to make space
            }
        }
        self.entries.push_back(entry);
    }
    
    /// Remove and return the entry from the front of the queue.
    pub fn dequeue(&mut self) -> Option<T> {
        self.entries.pop_front()
    }
    
    /// Peek at the entry at the front of the queue without removing it.
    pub fn peek(&self) -> Option<&T> {
        self.entries.front()
    }
    
    /// Get the number of entries in the queue.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    
    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    
    /// Get an iterator over the queue entries.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter()
    }
    
    /// Remove entries that don't match the predicate.
    pub fn retain(&mut self, predicate: impl Fn(&T) -> bool) {
        self.entries.retain(predicate);
    }
    
    /// Clear all entries from the queue.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A specialized dead-letter queue for webhook failures with automatic cleanup.
pub struct DeadLetterQueue {
    queue: WebhookQueue<DlqEntry>,
    retention_seconds: u64,
    max_capacity: Option<usize>,
}

impl DeadLetterQueue {
    /// Create a new DLQ with default retention (7 days) and no capacity limit.
    pub fn new() -> Self {
        DeadLetterQueue {
            queue: WebhookQueue::new(),
            retention_seconds: 7 * 24 * 3600, // 7 days
            max_capacity: None,
        }
    }
    
    /// Create a new DLQ with custom retention period.
    pub fn with_retention(retention_seconds: u64) -> Self {
        DeadLetterQueue {
            queue: WebhookQueue::new(),
            retention_seconds,
            max_capacity: None,
        }
    }
    
    /// Create a new DLQ with capacity limit.
    pub fn with_capacity(max_capacity: usize) -> Self {
        DeadLetterQueue {
            queue: WebhookQueue::with_capacity(max_capacity),
            retention_seconds: 7 * 24 * 3600,
            max_capacity: Some(max_capacity),
        }
    }
    
    /// Create a new DLQ with both retention and capacity limits.
    pub fn with_retention_and_capacity(retention_seconds: u64, max_capacity: usize) -> Self {
        DeadLetterQueue {
            queue: WebhookQueue::with_capacity_and_retention(max_capacity, retention_seconds),
            retention_seconds,
            max_capacity: Some(max_capacity),
        }
    }
    
    /// Add a failed delivery to the DLQ.
    pub fn add_failed_delivery(&mut self, entry: DlqEntry) {
        self.queue.enqueue(entry);
    }
    
    /// Get all entries in the DLQ.
    pub fn entries(&self) -> impl Iterator<Item = &DlqEntry> {
        self.queue.iter()
    }
    
    /// Get entries filtered by minimum status code and time range.
    pub fn query(&self, min_status: u16, from_ts: u64, to_ts: u64) -> Vec<&DlqEntry> {
        self.queue
            .iter()
            .filter(|e| {
                e.last_status_code >= min_status
                    && e.failed_at_timestamp >= from_ts
                    && e.failed_at_timestamp <= to_ts
            })
            .collect()
    }
    
    /// Remove expired entries based on retention policy.
    /// Returns the number of entries removed.
    pub fn cleanup_expired(&mut self, current_time: u64) -> usize {
        let before = self.queue.len();
        let cutoff = current_time.saturating_sub(self.retention_seconds);
        self.queue.retain(|e| e.failed_at_timestamp >= cutoff);
        before - self.queue.len()
    }
    
    /// Remove a specific entry by index.
    pub fn remove_entry(&mut self, index: usize) -> Option<DlqEntry> {
        if index < self.queue.len() {
            let mut entries: alloc::vec::Vec<DlqEntry> = self.queue.iter().cloned().collect();
            let removed = entries.remove(index);
            self.queue.clear();
            for entry in entries {
                self.queue.enqueue(entry);
            }
            Some(removed)
        } else {
            None
        }
    }
    
    /// Get the number of entries in the DLQ.
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    
    /// Check if the DLQ is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    
    /// Clear all entries from the DLQ.
    pub fn clear(&mut self) {
        self.queue.clear()
    }
    
    /// Get the retention period in seconds.
    pub fn retention_seconds(&self) -> u64 {
        self.retention_seconds
    }
    
    /// Get the maximum capacity if set.
    pub fn max_capacity(&self) -> Option<usize> {
        self.max_capacity
    }
}

/// Wrapper for backward compatibility with existing DLQ storage format.
/// Maintains the `BTreeMap<String, Vec<DlqEntry>>` interface while using
/// the new `DeadLetterQueue` internally for better queue management.
pub struct DlqStorage {
    queues: alloc::collections::BTreeMap<String, DeadLetterQueue>,
    default_retention_seconds: u64,
    default_max_capacity: Option<usize>,
}

impl DlqStorage {
    /// Create a new DLQ storage with default settings.
    pub fn new() -> Self {
        DlqStorage {
            queues: alloc::collections::BTreeMap::new(),
            default_retention_seconds: 7 * 24 * 3600,
            default_max_capacity: None,
        }
    }
    
    /// Create a new DLQ storage with custom defaults.
    pub fn with_defaults(retention_seconds: u64, max_capacity: Option<usize>) -> Self {
        DlqStorage {
            queues: alloc::collections::BTreeMap::new(),
            default_retention_seconds: retention_seconds,
            default_max_capacity: max_capacity,
        }
    }
    
    /// Add a failed delivery to a specific queue.
    pub fn add_failed_delivery(&mut self, queue_key: &str, entry: DlqEntry) {
        let queue = self.queues
            .entry(queue_key.to_string())
            .or_insert_with(|| {
                if let Some(capacity) = self.default_max_capacity {
                    DeadLetterQueue::with_retention_and_capacity(self.default_retention_seconds, capacity)
                } else {
                    DeadLetterQueue::with_retention(self.default_retention_seconds)
                }
            });
        queue.add_failed_delivery(entry);
    }
    
    /// Get entries from a specific queue.
    pub fn get_entries(&self, queue_key: &str) -> Vec<&DlqEntry> {
        self.queues
            .get(queue_key)
            .map(|queue| queue.entries().collect())
            .unwrap_or_default()
    }
    
    /// Query entries from a specific queue with filters.
    pub fn query_entries(&self, queue_key: &str, min_status: u16, from_ts: u64, to_ts: u64) -> Vec<&DlqEntry> {
        self.queues
            .get(queue_key)
            .map(|queue| queue.query(min_status, from_ts, to_ts))
            .unwrap_or_default()
    }
    
    /// Cleanup expired entries in all queues.
    /// Returns total number of entries removed across all queues.
    pub fn cleanup_all_expired(&mut self, current_time: u64) -> usize {
        self.queues
            .values_mut()
            .map(|queue| queue.cleanup_expired(current_time))
            .sum()
    }
    
    /// Cleanup expired entries in a specific queue.
    pub fn cleanup_expired(&mut self, queue_key: &str, current_time: u64) -> usize {
        self.queues
            .get_mut(queue_key)
            .map(|queue| queue.cleanup_expired(current_time))
            .unwrap_or(0)
    }
    
    /// Remove a specific entry from a queue.
    pub fn remove_entry(&mut self, queue_key: &str, index: usize) -> Option<DlqEntry> {
        self.queues
            .get_mut(queue_key)
            .and_then(|queue| queue.remove_entry(index))
    }
    
    /// Get the total number of entries across all queues.
    pub fn total_entries(&self) -> usize {
        self.queues.values().map(|queue| queue.len()).sum()
    }
    
    /// Convert to the legacy format for backward compatibility.
    pub fn to_legacy_format(&self) -> alloc::collections::BTreeMap<String, alloc::vec::Vec<DlqEntry>> {
        self.queues
            .iter()
            .map(|(key, queue)| (key.clone(), queue.entries().cloned().collect()))
            .collect()
    }
    
    /// Load from legacy format.
    pub fn from_legacy_format(map: alloc::collections::BTreeMap<String, alloc::vec::Vec<DlqEntry>>) -> Self {
        let mut storage = DlqStorage::new();
        for (key, entries) in map {
            let queue = DeadLetterQueue::new();
            for entry in entries {
                storage.add_failed_delivery(&key, entry);
            }
        }
        storage
    }
}

// ---------------------------------------------------------------------------
// DLQ entry
// ---------------------------------------------------------------------------

/// Structured record stored in the DLQ when all delivery attempts are exhausted.
///
/// The trace fields make a dead-lettered webhook traceable back to the request
/// that produced it: `trace_id` matches every log line emitted along the way,
/// and `last_attempt_span_id` points at the specific attempt that gave up.
#[derive(Clone, Debug, PartialEq)]
pub struct DlqEntry {
    /// The payload that failed to deliver.
    pub payload: String,
    /// Unix timestamp (seconds) when the entry was written to the DLQ.
    pub failed_at_timestamp: u64,
    /// Last HTTP status code received, or 0 if the transport failed entirely.
    pub last_status_code: u16,
    /// Number of delivery attempts made before giving up.
    pub attempts_made: u32,
    /// Human-readable description of the last error.
    pub last_error: String,
    /// Trace ID of the request this delivery belonged to.
    pub trace_id: String,
    /// Span ID of the delivery step (parent of every attempt span).
    pub span_id: String,
    /// Span ID of the final attempt — the one whose failure caused the
    /// dead-letter, and the span to search for in delivery logs.
    pub last_attempt_span_id: String,
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// Attempt to POST `payload` to `config.endpoint_url` with exponential backoff.
///
/// `http_post` is an injectable transport function `(url, body) -> Result<u16, String>`
/// that returns the HTTP status code on success or an error string on failure.
///
/// `sleep_fn` is called with the computed delay (ms) between retries.
///
/// `now_fn` returns the current Unix timestamp in seconds (used to timestamp DLQ entries).
///
/// When `config.signing_key` is `Some`, an `X-Anchor-Signature: sha256=<hex>`
/// header value is computed and passed as the third argument to `http_post`.
///
/// On total failure a [`DlqEntry`] is appended to `dlq` under
/// `config.dead_letter_storage_key` and an `AnchorKitError` is returned.
///
/// # Trace context
///
/// Callers that already hold a [`TraceContext`] should prefer
/// [`deliver_webhook_traced`], which propagates it into the delivery attempts,
/// the outbound headers, the logs and the DLQ entry. This function keeps its
/// original signature for existing call sites and derives a deterministic
/// context from the endpoint, the DLQ key and the payload, so a dead-lettered
/// webhook is still traceable even when the caller has no context to pass.
pub fn deliver_webhook<H, S, T>(
    config: &WebhookDeliveryConfig,
    payload: &str,
    dlq: &mut BTreeMap<String, Vec<DlqEntry>>,
    http_post: H,
    sleep_fn: S,
    now_fn: T,
) -> Result<(), AnchorKitError>
where
    H: Fn(&str, &str, Option<&str>) -> Result<u16, String>,
    S: FnMut(u64),
    T: Fn() -> u64,
{
    let trace = derive_delivery_trace(config, payload);
    deliver_webhook_traced(
        config,
        payload,
        &trace,
        dlq,
        |url, body, sig, _trace| http_post(url, body, sig),
        sleep_fn,
        now_fn,
    )
}

/// Derive a deterministic delivery trace for callers that supplied none.
///
/// Seeding on endpoint + DLQ key + payload means two deliveries of the same
/// payload to the same endpoint share a trace ID, which is the behaviour an
/// operator investigating a repeatedly failing webhook wants.
fn derive_delivery_trace(config: &WebhookDeliveryConfig, payload: &str) -> TraceContext {
    let seed = format!(
        "webhook:{}:{}:{}",
        config.endpoint_url, config.dead_letter_storage_key, payload
    );
    TraceContext::root_from_seed(&seed)
}

/// Deliver a webhook with backoff retry, propagating `trace` through every
/// attempt.
///
/// This is [`deliver_webhook`] with the trace context made explicit. It is the
/// entry point to use when a webhook is delivered as part of a larger request:
/// pass the request's context and the whole delivery — including each retry —
/// stays attached to that trace.
///
/// The context reaches four places:
///
/// 1. **The transport.** `http_post` receives the attempt's [`TraceContext`],
///    so it can attach `traceparent` / `X-Trace-Id` / `X-Span-Id` headers via
///    [`TraceContext::header_pairs`].
/// 2. **The attempt spans.** Attempt *n* runs under
///    `delivery_span.child_for_attempt(n)`, so retries are distinguishable but
///    share the trace ID.
/// 3. **The logs.** Every failed attempt logs its trace and span IDs.
/// 4. **The DLQ.** The resulting [`DlqEntry`] records the trace ID, the delivery
///    span and the span of the final attempt.
///
/// # Arguments
///
/// * `config` - Endpoint, retry parameters, DLQ key and optional signing key.
/// * `payload` - The request body to POST.
/// * `trace` - The context this delivery runs under. A `webhook-delivery` child
///   span is derived from it so the delivery is its own step in the trace.
/// * `dlq` - Dead-letter map; an entry is appended on total exhaustion.
/// * `http_post` - Injectable transport
///   `(url, body, signature_header, trace) -> Result<status, error>`.
/// * `sleep_fn` - Called with the backoff delay (ms) between attempts.
/// * `now_fn` - Returns the current Unix timestamp in seconds.
///
/// # Errors
///
/// Returns [`ErrorCode::WebhookDeliveryFailed`] when every attempt fails. The
/// error context includes the trace and span IDs so the failure can be
/// correlated with the delivery logs.
///
/// # Examples
///
/// ```rust
/// use std::collections::BTreeMap;
/// use anchorkit::retry::RetryConfig;
/// use anchorkit::trace_context::TraceContext;
/// use anchorkit::webhook::{deliver_webhook_traced, WebhookDeliveryConfig};
///
/// let config = WebhookDeliveryConfig {
///     endpoint_url: "https://example.com/hook".into(),
///     timeout_ms: 1_000,
///     retry_config: RetryConfig::new(3, 0, 0, 1),
///     dead_letter_storage_key: "hooks".into(),
///     signing_key: None,
///     max_payload_age_seconds: None,
///     require_nonce_for_replay_protection: false,
/// };
///
/// let request_trace = TraceContext::root_from_seed("deposit:txn-001");
/// let mut dlq = BTreeMap::new();
/// let mut header_values = Vec::new();
///
/// let result = deliver_webhook_traced(
///     &config,
///     r#"{"event":"deposit"}"#,
///     &request_trace,
///     &mut dlq,
///     |_url, _body, _sig, trace| {
///         header_values.push(trace.to_traceparent());
///         Ok(200)
///     },
///     |_ms| {},
///     || 1_000_000,
/// );
///
/// assert!(result.is_ok());
/// assert!(header_values[0].contains(request_trace.trace_id()));
/// ```
pub fn deliver_webhook_traced<H, S, T>(
    config: &WebhookDeliveryConfig,
    payload: &str,
    trace: &TraceContext,
    dlq: &mut BTreeMap<String, Vec<DlqEntry>>,
    http_post: H,
    mut sleep_fn: S,
    now_fn: T,
) -> Result<(), AnchorKitError>
where
    H: Fn(&str, &str, Option<&str>, &TraceContext) -> Result<u16, String>,
    S: FnMut(u64),
    T: Fn() -> u64,
{
    let retry_cfg = config.retry_config.clone();
    // Pre-compute signature header value (constant for a given payload+key).
    let sig_header: Option<String> = config.signing_key.as_ref().map(|k| {
        let hex = sign_payload(k, payload);
        alloc::format!("sha256={}", hex)
    });

    // The delivery is its own step under the caller's trace, so retries hang off
    // a span that means "webhook delivery" rather than off the caller directly.
    let delivery_span = trace.child("webhook-delivery");

    let last_error_msg: RefCell<String> = RefCell::new(String::new());
    let last_status: RefCell<u16> = RefCell::new(0);
    let last_attempt_span: RefCell<String> = RefCell::new(delivery_span.span_id().to_string());

    let mut jitter_source = crate::retry::LedgerJitterSource::new(0, now_fn());
    let result = retry_with_backoff_traced(
        &retry_cfg,
        &delivery_span,
        |_attempt, attempt_trace| {
            let sig_ref = sig_header.as_deref();
            *last_attempt_span.borrow_mut() = attempt_trace.span_id().to_string();

            let (status, msg) =
                match http_post(&config.endpoint_url, payload, sig_ref, attempt_trace) {
                    Ok(s) if s < 400 => return Ok(()),
                    Ok(s) => (s, format!("HTTP {s}")),
                    Err(e) => (0, e),
                };
            #[cfg(feature = "std")]
            std::eprintln!(
                "[webhook] {} attempt={} status={} error=\"{}\"",
                attempt_trace.log_fields(),
                _attempt + 1,
                status,
                msg
            );
            *last_error_msg.borrow_mut() = msg.clone();
            *last_status.borrow_mut() = status;
            Err(msg)
        },
        |_e: &String| true,
        &mut sleep_fn,
        &mut jitter_source,
    );

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let last = last_error_msg.into_inner();
            let status = last_status.into_inner();
            let last_span = last_attempt_span.into_inner();
            let attempts_made = config.retry_config.max_attempts;
            let entry = DlqEntry {
                payload: payload.to_string(),
                failed_at_timestamp: now_fn(),
                last_status_code: status,
                attempts_made,
                last_error: last.clone(),
                trace_id: delivery_span.trace_id().to_string(),
                span_id: delivery_span.span_id().to_string(),
                last_attempt_span_id: last_span.clone(),
            };
            dlq.entry(config.dead_letter_storage_key.clone())
                .or_default()
                .push(entry);

            Err(AnchorKitError::with_context(
                ErrorCode::WebhookDeliveryFailed,
                &format!(
                    "Webhook delivery failed after {} attempt(s): {}",
                    attempts_made, e
                ),
                &format!(
                    "attempts_made={} last_status={} last_error={} {} last_attempt_span_id={}",
                    attempts_made,
                    status,
                    last,
                    delivery_span.log_fields(),
                    last_span
                ),
            ))
        }
    }
}

/// Return the DLQ entries recorded under `key` that belong to `trace_id`.
///
/// The operator-facing counterpart to trace propagation: given a trace ID from
/// a log line, find every webhook that dead-lettered under that request.
///
/// # Examples
///
/// ```rust
/// use std::collections::BTreeMap;
/// use anchorkit::webhook::{dlq_entries_for_trace, DlqEntry};
///
/// let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
/// assert!(dlq_entries_for_trace(&dlq, "hooks", "abc").is_empty());
/// ```
pub fn dlq_entries_for_trace<'a>(
    dlq: &'a BTreeMap<String, Vec<DlqEntry>>,
    key: &str,
    trace_id: &str,
) -> Vec<&'a DlqEntry> {
    dlq.get(key)
        .map(|entries| entries.iter().filter(|e| e.trace_id == trace_id).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// DLQ inspection
// ---------------------------------------------------------------------------

/// Return all [`DlqEntry`] records stored under `key` in the DLQ, or an empty slice.
pub fn get_dead_letter_webhooks<'a>(
    dlq: &'a BTreeMap<String, Vec<DlqEntry>>,
    key: &str,
) -> &'a [DlqEntry] {
    dlq.get(key).map(Vec::as_slice).unwrap_or(&[])
}

/// Query DLQ entries filtered by minimum HTTP status code and time range.
///
/// Returns entries where `last_status_code >= min_status` (use 0 to match all)
/// and `failed_at_timestamp` is within `[from_ts, to_ts]` (inclusive).
/// Pass `to_ts = u64::MAX` to match all entries up to the present.
pub fn query_dlq<'a>(
    dlq: &'a BTreeMap<String, Vec<DlqEntry>>,
    key: &str,
    min_status: u16,
    from_ts: u64,
    to_ts: u64,
) -> Vec<&'a DlqEntry> {
    dlq.get(key)
        .map(|entries| {
            entries
                .iter()
                .filter(|e| {
                    e.last_status_code >= min_status
                        && e.failed_at_timestamp >= from_ts
                        && e.failed_at_timestamp <= to_ts
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::collections::BTreeMap;

    fn make_config(max_retries: u32) -> WebhookDeliveryConfig {
        WebhookDeliveryConfig {
            endpoint_url: "https://example.com/hook".to_string(),
            timeout_ms: 1000,
            retry_config: RetryConfig {
                max_attempts: max_retries,
                base_delay_ms: 1,
                backoff_multiplier: 1,
                max_delay_ms: 10,
                strategy: crate::retry::BackoffStrategy::Exponential,
                jitter_policy: crate::retry::JitterPolicy::None,
            },
            dead_letter_storage_key: "test-key".to_string(),
            signing_key: None,
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        }
    }

    /// A DLQ entry with placeholder trace fields, for tests that only exercise
    /// the query helpers.
    fn dlq_entry(payload: &str, failed_at_timestamp: u64, last_status_code: u16) -> DlqEntry {
        DlqEntry {
            payload: payload.to_string(),
            failed_at_timestamp,
            last_status_code,
            attempts_made: 1,
            last_error: "e".to_string(),
            trace_id: "0".repeat(31) + "1",
            span_id: "0".repeat(15) + "1",
            last_attempt_span_id: "0".repeat(15) + "1",
        }
    }

    #[test]
    fn deliver_succeeds_on_first_attempt() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &make_config(3),
            "payload",
            &mut dlq,
            |_, _, _| Ok(200),
            |_| {},
            || 1000,
        );
        assert!(result.is_ok());
        assert!(dlq.is_empty());
    }

    #[test]
    fn deliver_stores_dlq_entry_after_exhaustion() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &make_config(2),
            "my-payload",
            &mut dlq,
            |_, _, _| Ok(503),
            |_| {},
            || 9999,
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::WebhookDeliveryFailed);

        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.payload, "my-payload");
        assert_eq!(entry.last_status_code, 503);
        assert_eq!(entry.attempts_made, 2);
        assert_eq!(entry.failed_at_timestamp, 9999);
        assert!(!entry.last_error.is_empty());
    }

    #[test]
    fn deliver_stores_dlq_entry_on_transport_error() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &make_config(1),
            "payload",
            &mut dlq,
            |_, _, _| Err("connection refused".to_string()),
            |_| {},
            || 42,
        );
        assert!(result.is_err());
        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].last_status_code, 0); // transport failure
        assert_eq!(entries[0].attempts_made, 1);
    }

    #[test]
    fn multiple_failures_accumulate_in_dlq() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let config = make_config(1);
        for i in 0..3u64 {
            let _ = deliver_webhook(
                &config,
                &alloc::format!("payload-{}", i),
                &mut dlq,
                |_, _, _| Ok(500),
                |_| {},
                move || i * 100,
            );
        }
        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn query_dlq_filters_by_status_and_time() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let key = "test-key";
        dlq.entry(key.to_string()).or_default().extend([
            dlq_entry("a", 100, 500),
            dlq_entry("b", 200, 503),
            dlq_entry("c", 300, 0),
        ]);

        // All entries
        assert_eq!(query_dlq(&dlq, key, 0, 0, u64::MAX).len(), 3);
        // Only 5xx
        assert_eq!(query_dlq(&dlq, key, 500, 0, u64::MAX).len(), 2);
        // Time range
        assert_eq!(query_dlq(&dlq, key, 0, 150, 250).len(), 1);
        // No match
        assert_eq!(query_dlq(&dlq, key, 0, 400, 500).len(), 0);
    }

    // -----------------------------------------------------------------------
    // Issue #610 — trace context propagation across delivery attempts
    // -----------------------------------------------------------------------

    /// Every delivery attempt, including retries, carries the caller's trace ID.
    #[test]
    fn trace_id_survives_every_delivery_attempt() {
        let trace = TraceContext::root_from_seed("deposit:txn-001");
        let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook_traced(
            &make_config(3),
            "payload",
            &trace,
            &mut dlq,
            |_url, _body, _sig, attempt_trace| {
                seen.borrow_mut().push(attempt_trace.trace_id().to_string());
                Ok(503)
            },
            |_| {},
            || 1000,
        );

        assert!(result.is_err());
        let seen = seen.into_inner();
        assert_eq!(seen.len(), 3, "all three attempts should have run");
        assert!(
            seen.iter().all(|id| id == trace.trace_id()),
            "trace_id must not change across delivery retries: {seen:?}"
        );
    }

    /// Attempts are individually identifiable: distinct spans, all parented to
    /// the delivery span.
    #[test]
    fn each_delivery_attempt_has_its_own_span() {
        let trace = TraceContext::root_from_seed("deposit:txn-002");
        let delivery_span = trace.child("webhook-delivery");
        let spans: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let _ = deliver_webhook_traced(
            &make_config(3),
            "payload",
            &trace,
            &mut dlq,
            |_url, _body, _sig, attempt_trace| {
                assert_eq!(
                    attempt_trace.parent_span_id(),
                    Some(delivery_span.span_id()),
                    "attempt spans hang off the delivery span"
                );
                spans.borrow_mut().push(attempt_trace.span_id().to_string());
                Ok(503)
            },
            |_| {},
            || 1000,
        );

        let spans = spans.into_inner();
        assert_eq!(spans.len(), 3);
        assert_ne!(spans[0], spans[1]);
        assert_ne!(spans[1], spans[2]);
        assert_ne!(spans[0], spans[2]);
    }

    /// A successful retry stops the loop but still ran under the same trace.
    #[test]
    fn trace_survives_a_delivery_that_succeeds_on_retry() {
        let trace = TraceContext::root_from_seed("deposit:txn-003");
        let attempts: RefCell<u32> = RefCell::new(0);
        let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook_traced(
            &make_config(4),
            "payload",
            &trace,
            &mut dlq,
            |_url, _body, _sig, attempt_trace| {
                seen.borrow_mut().push(attempt_trace.trace_id().to_string());
                let mut n = attempts.borrow_mut();
                *n += 1;
                if *n < 3 {
                    Ok(500)
                } else {
                    Ok(200)
                }
            },
            |_| {},
            || 1000,
        );

        assert!(result.is_ok());
        assert!(dlq.is_empty(), "no DLQ entry when a retry succeeds");
        let seen = seen.into_inner();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|id| id == trace.trace_id()));
    }

    /// The DLQ entry records the trace, so a dead-lettered webhook can be
    /// traced back to the request that produced it.
    #[test]
    fn dlq_entry_records_the_trace_context() {
        let trace = TraceContext::root_from_seed("deposit:txn-004");
        let delivery_span = trace.child("webhook-delivery");
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook_traced(
            &make_config(2),
            "my-payload",
            &trace,
            &mut dlq,
            |_url, _body, _sig, _t| Ok(503),
            |_| {},
            || 9999,
        );

        assert!(result.is_err());
        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.trace_id, trace.trace_id());
        assert_eq!(entry.span_id, delivery_span.span_id());
        assert_eq!(
            entry.last_attempt_span_id,
            delivery_span.child_for_attempt(1).span_id(),
            "the final attempt's span is the one recorded"
        );
    }

    /// The returned error carries the trace context in its diagnostic detail.
    #[test]
    fn delivery_error_context_includes_the_trace() {
        let trace = TraceContext::root_from_seed("deposit:txn-005");
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let err = deliver_webhook_traced(
            &make_config(2),
            "payload",
            &trace,
            &mut dlq,
            |_url, _body, _sig, _t| Err("connection refused".to_string()),
            |_| {},
            || 1000,
        )
        .unwrap_err();

        let detail = alloc::format!("{:?}", err);
        assert!(
            detail.contains(trace.trace_id()),
            "error detail should name the trace: {detail}"
        );
    }

    /// `dlq_entries_for_trace` finds exactly the entries for one trace.
    #[test]
    fn dlq_entries_can_be_looked_up_by_trace_id() {
        let config = make_config(1);
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let wanted = TraceContext::root_from_seed("wanted");
        let other = TraceContext::root_from_seed("other");
        for trace in [&wanted, &other, &wanted] {
            let _ = deliver_webhook_traced(
                &config,
                "payload",
                trace,
                &mut dlq,
                |_url, _body, _sig, _t| Ok(500),
                |_| {},
                || 1000,
            );
        }

        assert_eq!(get_dead_letter_webhooks(&dlq, "test-key").len(), 3);
        let found = dlq_entries_for_trace(&dlq, "test-key", wanted.trace_id());
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|e| e.trace_id == wanted.trace_id()));
    }

    /// The untraced entry point still produces a traceable DLQ entry, so
    /// existing callers gain correlation without changing their code.
    #[test]
    fn untraced_delivery_still_records_a_valid_trace() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let _ = deliver_webhook(
            &make_config(1),
            "payload",
            &mut dlq,
            |_, _, _| Ok(500),
            |_| {},
            || 1000,
        );

        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.trace_id.len(), crate::trace_context::TRACE_ID_HEX_LEN);
        assert_eq!(entry.span_id.len(), crate::trace_context::SPAN_ID_HEX_LEN);
        assert_ne!(entry.span_id, entry.last_attempt_span_id);
    }

    /// The derived context is stable: the same payload to the same endpoint
    /// dead-letters under the same trace ID every time.
    #[test]
    fn untraced_delivery_trace_is_stable_across_runs() {
        let config = make_config(1);
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        for _ in 0..2 {
            let _ = deliver_webhook(
                &config,
                "payload",
                &mut dlq,
                |_, _, _| Ok(500),
                |_| {},
                || 1000,
            );
        }

        let entries = get_dead_letter_webhooks(&dlq, "test-key");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].trace_id, entries[1].trace_id);
    }
}
