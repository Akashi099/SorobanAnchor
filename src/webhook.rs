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
    retry::{retry_with_backoff, RetryConfig},
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
pub fn deliver_webhook<H, S, T>(
    config: &WebhookDeliveryConfig,
    payload: &str,
    dlq: &mut BTreeMap<String, Vec<DlqEntry>>,
    http_post: H,
    mut sleep_fn: S,
    now_fn: T,
) -> Result<(), AnchorKitError>
where
    H: Fn(&str, &str, Option<&str>) -> Result<u16, String>,
    S: FnMut(u64),
    T: Fn() -> u64,
{
    let retry_cfg = config.retry_config.clone();
    // Pre-compute signature header value (constant for a given payload+key).
    let sig_header: Option<String> = config.signing_key.as_ref().map(|k| {
        let hex = sign_payload(k, payload);
        alloc::format!("sha256={}", hex)
    });

    let last_error_msg: RefCell<String> = RefCell::new(String::new());
    let last_status: RefCell<u16> = RefCell::new(0);

    let mut jitter_source = crate::retry::LedgerJitterSource::new(0, now_fn());
    let result = retry_with_backoff(
        &retry_cfg,
        |attempt| {
            let sig_ref = sig_header.as_deref();
            let (status, msg) = match http_post(&config.endpoint_url, payload, sig_ref) {
                Ok(s) if s < 400 => return Ok(()),
                Ok(s) => (s, format!("HTTP {s}")),
                Err(e) => (0, e),
            };
            #[cfg(feature = "std")]
            std::eprintln!(
                "[webhook] attempt={} status={} error=\"{}\"",
                attempt + 1,
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
            let attempts_made = config.retry_config.max_attempts;
            let entry = DlqEntry {
                payload: payload.to_string(),
                failed_at_timestamp: now_fn(),
                last_status_code: status,
                attempts_made,
                last_error: last.clone(),
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
                &format!("attempts_made={} last_status={} last_error={}", attempts_made, status, last),
            ))
        }
    }
}

/// Like [`deliver_webhook`], additionally recording delivery metrics.
///
/// Emitted metrics (see [`crate::metrics::names`]):
///
/// * [`names::WEBHOOK_DELIVERIES`] — one per call.
/// * [`names::WEBHOOK_ATTEMPTS`] — one per HTTP POST attempt, retries included.
/// * [`names::WEBHOOK_SUCCESSES`] / [`names::WEBHOOK_FAILURES`] — final outcome.
/// * [`names::WEBHOOK_DLQ_ENTRIES`] — counter, one per entry written to the DLQ.
/// * [`names::WEBHOOK_DLQ_DEPTH`] — gauge, total entries across all DLQ keys
///   after this delivery (a resource-usage signal for DLQ growth).
///
/// Delegates to [`deliver_webhook`] so delivery semantics stay identical.
///
/// [`names::WEBHOOK_DELIVERIES`]: crate::metrics::names::WEBHOOK_DELIVERIES
/// [`names::WEBHOOK_ATTEMPTS`]: crate::metrics::names::WEBHOOK_ATTEMPTS
/// [`names::WEBHOOK_SUCCESSES`]: crate::metrics::names::WEBHOOK_SUCCESSES
/// [`names::WEBHOOK_FAILURES`]: crate::metrics::names::WEBHOOK_FAILURES
/// [`names::WEBHOOK_DLQ_ENTRIES`]: crate::metrics::names::WEBHOOK_DLQ_ENTRIES
/// [`names::WEBHOOK_DLQ_DEPTH`]: crate::metrics::names::WEBHOOK_DLQ_DEPTH
pub fn deliver_webhook_metered<H, S, T>(
    config: &WebhookDeliveryConfig,
    payload: &str,
    dlq: &mut BTreeMap<String, Vec<DlqEntry>>,
    http_post: H,
    sleep_fn: S,
    now_fn: T,
    metrics: &crate::metrics::MetricsRegistry,
) -> Result<(), AnchorKitError>
where
    H: Fn(&str, &str, Option<&str>) -> Result<u16, String>,
    S: FnMut(u64),
    T: Fn() -> u64,
{
    use crate::metrics::names;

    metrics.incr(names::WEBHOOK_DELIVERIES);
    let result = deliver_webhook(
        config,
        payload,
        dlq,
        |url, body, sig| {
            metrics.incr(names::WEBHOOK_ATTEMPTS);
            http_post(url, body, sig)
        },
        sleep_fn,
        now_fn,
    );
    match &result {
        Ok(()) => metrics.incr(names::WEBHOOK_SUCCESSES),
        Err(_) => {
            metrics.incr(names::WEBHOOK_FAILURES);
            // deliver_webhook appends exactly one DLQ entry per failed delivery.
            metrics.incr(names::WEBHOOK_DLQ_ENTRIES);
        }
    }
    let dlq_depth: usize = dlq.values().map(Vec::len).sum();
    metrics.set_gauge(names::WEBHOOK_DLQ_DEPTH, dlq_depth as u64);
    result
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
            DlqEntry { payload: "a".to_string(), failed_at_timestamp: 100, last_status_code: 500, attempts_made: 1, last_error: "e".to_string() },
            DlqEntry { payload: "b".to_string(), failed_at_timestamp: 200, last_status_code: 503, attempts_made: 1, last_error: "e".to_string() },
            DlqEntry { payload: "c".to_string(), failed_at_timestamp: 300, last_status_code: 0,   attempts_made: 1, last_error: "e".to_string() },
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
}
