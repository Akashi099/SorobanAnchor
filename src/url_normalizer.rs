//! URL normalization and allowlist/blocklist enforcement.
//! URL normalization and allowlist/blocklist enforcement for anchor URLs.
//!
//! Different anchors may present semantically identical URLs in slightly
//! different textual forms (trailing slash, default port, mixed case). This
//! module normalizes URLs to a canonical form so that caching, comparison, and
//! validation produce consistent results regardless of how the caller formatted
//! the URL.
//!
//! It also provides a configurable [`UrlFilterPolicy`] that supports:
//! - An **allowlist** of approved hostname suffixes or exact hostnames.
//! - A **blocklist** of rejected hostname suffixes or exact hostnames.
//!
//! ## Normalization rules
//!
//! 1. Lowercase the scheme and hostname.
//! 2. Remove the default port (443 for `https`, 80 for `http`).
//! 3. Strip a single trailing `/` from the path **when the path is exactly
//!    `/`** (bare root), so `https://example.com/` → `https://example.com`.
//!    Paths with real segments are left intact.
//! 4. Preserve the path, query string, and fragment unmodified except for
//!    rule 3.
//!
//! Callers should pass the normalized URL to [`validate_anchor_domain`] and to
//! any caching or HTTP-client call.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::errors::AnchorKitError;

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

/// Normalize a URL to a canonical form.
///
/// Applies the following transforms in order:
/// 1. Trim leading/trailing ASCII whitespace.
/// 2. Lowercase the scheme and host portions only (path/query/fragment are
///    preserved as-is to avoid breaking case-sensitive paths).
/// 3. Remove the default port (`:443` for `https://`, `:80` for `http://`).
/// 4. Strip a bare trailing slash (`/`) from the authority when no path
///    segment follows it — `https://example.com/` → `https://example.com`.
///
/// Returns `Err(InvalidEndpointFormat)` when the URL does not contain `://`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::url_normalizer::normalize_url;
///
/// assert_eq!(normalize_url("HTTPS://Example.COM/").unwrap(),   "https://example.com");
/// assert_eq!(normalize_url("https://anchor.example.com:443").unwrap(), "https://anchor.example.com");
/// assert_eq!(normalize_url("https://anchor.example.com:8080").unwrap(), "https://anchor.example.com:8080");
/// assert_eq!(normalize_url("https://anchor.example.com/sep6/").unwrap(), "https://anchor.example.com/sep6/");
/// ```
pub fn normalize_url(url: &str) -> Result<String, AnchorKitError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Split on "://" to separate scheme from the rest.
    let sep = match trimmed.find("://") {
        Some(pos) => pos,
        None => return Err(AnchorKitError::invalid_endpoint_format()),
    };

    let scheme = &trimmed[..sep];
    let after_scheme = &trimmed[sep + 3..]; // skip "://"

    // Lowercase the scheme.
    let scheme_lower: String = scheme.to_ascii_lowercase();

    // Determine the default port for this scheme so we can strip it.
    let default_port: Option<u16> = match scheme_lower.as_str() {
        "https" => Some(443),
        "http"  => Some(80),
        _       => None,
    };

    // Find where the authority ends (first '/', '?', '#' after the scheme).
    let (authority_raw, rest) = match after_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => (&after_scheme[..pos], &after_scheme[pos..]),
        None      => (after_scheme, ""),
    };

    // Lowercase the authority (host + optional port).
    let authority_lower: String = authority_raw.to_ascii_lowercase();

    // Strip the default port from the authority when present.
    let authority_normalised: String = if let Some(def_port) = default_port {
        let port_suffix = alloc::format!(":{}", def_port);
        if authority_lower.ends_with(&port_suffix) {
            authority_lower[..authority_lower.len() - port_suffix.len()].to_string()
        } else {
            authority_lower
        }
    } else {
        authority_lower
    };

    // Strip a bare trailing slash — only when `rest == "/"` with nothing after.
    let rest_normalised = if rest == "/" { "" } else { rest };

    Ok(alloc::format!("{}://{}{}", scheme_lower, authority_normalised, rest_normalised))
}

/// Extract only the hostname (without port) from a URL.
///
/// Returns `None` when the URL does not contain `://` or when the authority is
/// empty after splitting.
///
/// Lowercases the returned hostname.
///
/// # Examples
///
/// ```rust
/// use anchorkit::url_normalizer::extract_hostname;
///
/// assert_eq!(extract_hostname("https://Anchor.Example.COM:8080/path"), Some("anchor.example.com".to_string()));
/// assert_eq!(extract_hostname("https://example.com"), Some("example.com".to_string()));
/// assert_eq!(extract_hostname("not-a-url"), None);
/// ```
pub fn extract_hostname(url: &str) -> Option<String> {
    let sep = url.find("://")?;
    let after_scheme = &url[sep + 3..];
    let authority = match after_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => &after_scheme[..pos],
        None      => after_scheme,
    };
    if authority.is_empty() {
        return None;
    }
    // Strip port.
    let host = if let Some(colon) = authority.rfind(':') {
        let port_str = &authority[colon + 1..];
        if port_str.chars().all(|c| c.is_ascii_digit()) {
            &authority[..colon]
        } else {
            authority
        }
    } else {
        authority
    };
    Some(host.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// UrlFilterPolicy — allowlist + blocklist
// ---------------------------------------------------------------------------

/// A single entry in a URL filter list.
///
/// Matching is **suffix-based on the hostname**:
/// - `"example.com"` matches `example.com` and any subdomain like
///   `api.example.com`.
/// - `"evil.example.com"` matches `evil.example.com` and its subdomains but
///   **not** `safe.example.com`.
///
/// Exact matching (no subdomains) can be forced by setting `exact = true`.
#[derive(Clone, Debug, PartialEq)]
pub struct UrlFilterEntry {
    /// The hostname pattern (lowercased). Leading dot is optional;
    /// `"example.com"` and `".example.com"` are treated identically.
    pub pattern: String,
    /// When `true` only the exact hostname matches; subdomains are excluded.
    pub exact: bool,
}

impl UrlFilterEntry {
    /// Construct a suffix-match entry.
    pub fn suffix(pattern: &str) -> Self {
        UrlFilterEntry {
            pattern: pattern.trim_start_matches('.').to_ascii_lowercase(),
            exact: false,
        }
    }

    /// Construct an exact-match entry.
    pub fn exact(hostname: &str) -> Self {
        UrlFilterEntry {
            pattern: hostname.to_ascii_lowercase(),
            exact: true,
        }
    }

    /// Return `true` when `hostname` (already lowercased) matches this entry.
    pub fn matches(&self, hostname: &str) -> bool {
        if self.exact {
            hostname == self.pattern.as_str()
        } else {
            // Suffix match: hostname ends with pattern, and the character just
            // before the pattern (if any) is a dot — so "notexample.com" does
            // not match ".example.com".
            if hostname == self.pattern.as_str() {
                return true;
            }
            if hostname.ends_with(self.pattern.as_str()) {
                let prefix_len = hostname.len() - self.pattern.len();
                return hostname.as_bytes().get(prefix_len.wrapping_sub(1)) == Some(&b'.');
            }
            false
        }
    }
}

/// Configurable URL filter policy combining an allowlist and a blocklist.
///
/// Evaluation order:
/// 1. If the **blocklist** is non-empty and the hostname matches any blocked
///    entry → **reject** (returns `Err`).
/// 2. If the **allowlist** is non-empty and the hostname does NOT match any
///    allowed entry → **reject** (returns `Err`).
/// 3. Otherwise → **accept** (returns `Ok`).
///
/// An empty allowlist means "allow any host not blocked".
/// An empty blocklist means "block nothing explicitly".
#[derive(Clone, Debug, Default)]
pub struct UrlFilterPolicy {
    pub allowlist: Vec<UrlFilterEntry>,
    pub blocklist: Vec<UrlFilterEntry>,
}

impl UrlFilterPolicy {
    /// Create an empty (permissive) policy that accepts all URLs.
    pub fn new() -> Self {
        UrlFilterPolicy::default()
    }

    /// Add a suffix-based allowlist entry.
    pub fn allow_suffix(mut self, pattern: &str) -> Self {
        self.allowlist.push(UrlFilterEntry::suffix(pattern));
        self
    }

    /// Add an exact-match allowlist entry.
    pub fn allow_exact(mut self, hostname: &str) -> Self {
        self.allowlist.push(UrlFilterEntry::exact(hostname));
        self
    }

    /// Add a suffix-based blocklist entry.
    pub fn block_suffix(mut self, pattern: &str) -> Self {
        self.blocklist.push(UrlFilterEntry::suffix(pattern));
        self
    }

    /// Add an exact-match blocklist entry.
    pub fn block_exact(mut self, hostname: &str) -> Self {
        self.blocklist.push(UrlFilterEntry::exact(hostname));
        self
    }

    /// Evaluate the policy against a full URL.
    ///
    /// The URL is normalized before the hostname is extracted so that
    /// mixed-case and default-port variants are handled consistently.
    ///
    /// Returns `Ok(())` when the URL is permitted.
    /// Returns `Err(InvalidEndpointFormat)` with a human-readable context
    /// message when the URL is blocked or not on the allowlist.
    pub fn check(&self, url: &str) -> Result<(), AnchorKitError> {
        let normalized = normalize_url(url)?;
        let hostname = extract_hostname(&normalized)
            .ok_or_else(AnchorKitError::invalid_endpoint_format)?;

        // 1. Blocklist check — explicit rejection takes precedence.
        for entry in &self.blocklist {
            if entry.matches(&hostname) {
                return Err(AnchorKitError::with_context(
                    crate::errors::ErrorCode::InvalidEndpointFormat,
                    "host is on the blocklist",
                    &hostname,
                ));
            }
        }

        // 2. Allowlist check — when non-empty the host must appear on it.
        if !self.allowlist.is_empty() {
            let allowed = self.allowlist.iter().any(|e| e.matches(&hostname));
            if !allowed {
                return Err(AnchorKitError::with_context(
                    crate::errors::ErrorCode::InvalidEndpointFormat,
                    "host is not on the allowlist",
                    &hostname,
                ));
            }
        }

        Ok(())
    }
}

/// Convenience wrapper: normalize then validate a URL with the given filter
/// policy and the core [`validate_anchor_domain`] rules.
///
/// This is the recommended single call for all outbound URL handling:
/// normalization, structural validation, and filter-policy enforcement are
/// applied in one shot.
///
/// # Arguments
///
/// * `url`    — Raw URL supplied by the caller (may have mixed case, default
///              port, trailing slash, etc.).
/// * `policy` — Active [`UrlFilterPolicy`]. Pass `&UrlFilterPolicy::new()` for
///              no additional filtering beyond the core structural rules.
///
/// # Returns
///
/// `Ok(normalized_url)` — the canonical URL string ready for use.
/// `Err(AnchorKitError)` — structural or policy violation.
///
/// # Examples
///
/// ```rust
/// use anchorkit::url_normalizer::{normalize_and_validate, UrlFilterPolicy};
///
/// let policy = UrlFilterPolicy::new().allow_suffix("example.com");
/// let result = normalize_and_validate("HTTPS://Anchor.Example.COM:443/sep6", &policy);
/// assert_eq!(result.unwrap(), "https://anchor.example.com/sep6");
///
/// let blocked = normalize_and_validate("https://evil.example.org/sep6", &policy);
/// assert!(blocked.is_err());
/// ```
pub fn normalize_and_validate(url: &str, policy: &UrlFilterPolicy) -> Result<String, AnchorKitError> {
    let normalized = normalize_url(url)?;
    // Re-use the existing structural validator on the already-normalized URL.
    crate::domain_validator::validate_anchor_domain(&normalized)?;
    // Apply filter policy.
    policy.check(&normalized)?;
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_url ─────────────────────────────────────────────────────

    #[test]
    fn test_normalize_lowercase_scheme_and_host() {
        assert_eq!(
            normalize_url("HTTPS://ANCHOR.EXAMPLE.COM").unwrap(),
            "https://anchor.example.com"
        );
        assert_eq!(
            normalize_url("Https://Api.Example.Com/path").unwrap(),
            "https://api.example.com/path"
        );
    }

    #[test]
    fn test_normalize_strips_default_https_port() {
        assert_eq!(
            normalize_url("https://anchor.example.com:443").unwrap(),
            "https://anchor.example.com"
        );
        assert_eq!(
            normalize_url("https://anchor.example.com:443/sep6").unwrap(),
            "https://anchor.example.com/sep6"
        );
    }

    #[test]
    fn test_normalize_strips_default_http_port() {
        assert_eq!(
            normalize_url("http://example.com:80/sep6").unwrap(),
            "http://example.com/sep6"
        );
    }

    #[test]
    fn test_normalize_preserves_non_default_port() {
        assert_eq!(
            normalize_url("https://example.com:8080").unwrap(),
            "https://example.com:8080"
        );
        assert_eq!(
            normalize_url("https://example.com:8443/sep6").unwrap(),
            "https://example.com:8443/sep6"
        );
    }

    #[test]
    fn test_normalize_strips_bare_trailing_slash() {
        assert_eq!(
            normalize_url("https://example.com/").unwrap(),
            "https://example.com"
        );
        // Port + bare slash
        assert_eq!(
            normalize_url("https://example.com:443/").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn test_normalize_preserves_path_trailing_slash() {
        // A trailing slash on an actual path segment is kept.
        assert_eq!(
            normalize_url("https://example.com/sep6/").unwrap(),
            "https://example.com/sep6/"
        );
    }

    #[test]
    fn test_normalize_preserves_path_case() {
        // Only scheme + host are lowercased; path is not altered.
        assert_eq!(
            normalize_url("https://Example.COM/MyPath?Query=Value").unwrap(),
            "https://example.com/MyPath?Query=Value"
        );
    }

    #[test]
    fn test_normalize_trims_whitespace() {
        assert_eq!(
            normalize_url("  https://example.com  ").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn test_normalize_empty_returns_error() {
        assert!(normalize_url("").is_err());
        assert!(normalize_url("   ").is_err());
    }

    #[test]
    fn test_normalize_missing_scheme_separator_returns_error() {
        assert!(normalize_url("example.com").is_err());
        assert!(normalize_url("https:/example.com").is_err());
    }

    #[test]
    fn test_normalize_query_and_fragment_preserved() {
        assert_eq!(
            normalize_url("HTTPS://Example.com?foo=bar#section").unwrap(),
            "https://example.com?foo=bar#section"
        );
    }

    // ── extract_hostname ──────────────────────────────────────────────────

    #[test]
    fn test_extract_hostname_basic() {
        assert_eq!(
            extract_hostname("https://anchor.example.com"),
            Some("anchor.example.com".to_string())
        );
    }

    #[test]
    fn test_extract_hostname_strips_port() {
        assert_eq!(
            extract_hostname("https://anchor.example.com:8080/path"),
            Some("anchor.example.com".to_string())
        );
    }

    #[test]
    fn test_extract_hostname_lowercases() {
        assert_eq!(
            extract_hostname("HTTPS://ANCHOR.EXAMPLE.COM:443"),
            Some("anchor.example.com".to_string())
        );
    }

    #[test]
    fn test_extract_hostname_no_scheme_returns_none() {
        assert_eq!(extract_hostname("example.com"), None);
    }

    // ── UrlFilterEntry ────────────────────────────────────────────────────

    #[test]
    fn test_filter_entry_suffix_matches_exact() {
        let e = UrlFilterEntry::suffix("example.com");
        assert!(e.matches("example.com"));
    }

    #[test]
    fn test_filter_entry_suffix_matches_subdomain() {
        let e = UrlFilterEntry::suffix("example.com");
        assert!(e.matches("api.example.com"));
        assert!(e.matches("deep.sub.example.com"));
    }

    #[test]
    fn test_filter_entry_suffix_does_not_match_partial() {
        let e = UrlFilterEntry::suffix("example.com");
        // "notexample.com" must NOT match ".example.com"
        assert!(!e.matches("notexample.com"));
        assert!(!e.matches("badexample.com"));
    }

    #[test]
    fn test_filter_entry_exact_matches_only_exact() {
        let e = UrlFilterEntry::exact("example.com");
        assert!(e.matches("example.com"));
        assert!(!e.matches("api.example.com"));
        assert!(!e.matches("other.com"));
    }

    #[test]
    fn test_filter_entry_leading_dot_stripped() {
        // ".example.com" and "example.com" are treated the same.
        let e = UrlFilterEntry::suffix(".example.com");
        assert!(e.matches("example.com"));
        assert!(e.matches("api.example.com"));
    }

    // ── UrlFilterPolicy ───────────────────────────────────────────────────

    #[test]
    fn test_policy_empty_accepts_all() {
        let policy = UrlFilterPolicy::new();
        assert!(policy.check("https://anything.example.com").is_ok());
        assert!(policy.check("https://other.org").is_ok());
    }

    #[test]
    fn test_policy_allowlist_accepts_matching_host() {
        let policy = UrlFilterPolicy::new().allow_suffix("example.com");
        assert!(policy.check("https://anchor.example.com").is_ok());
        assert!(policy.check("https://api.example.com/sep6").is_ok());
    }

    #[test]
    fn test_policy_allowlist_rejects_non_matching_host() {
        let policy = UrlFilterPolicy::new().allow_suffix("example.com");
        let err = policy.check("https://other.org").unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidEndpointFormat);
        assert!(err.message.contains("allowlist"));
    }

    #[test]
    fn test_policy_blocklist_rejects_blocked_host() {
        let policy = UrlFilterPolicy::new().block_suffix("evil.example.com");
        let err = policy.check("https://evil.example.com/path").unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidEndpointFormat);
        assert!(err.message.contains("blocklist"));
    }

    #[test]
    fn test_policy_blocklist_allows_non_blocked_host() {
        let policy = UrlFilterPolicy::new().block_suffix("evil.example.com");
        assert!(policy.check("https://safe.example.com").is_ok());
    }

    #[test]
    fn test_policy_blocklist_takes_precedence_over_allowlist() {
        // Host is on both — blocklist wins.
        let policy = UrlFilterPolicy::new()
            .allow_suffix("example.com")
            .block_exact("evil.example.com");
        let err = policy.check("https://evil.example.com").unwrap_err();
        assert!(err.message.contains("blocklist"));
    }

    #[test]
    fn test_policy_normalizes_before_checking() {
        // Mixed-case and default-port URL should be normalized and accepted.
        let policy = UrlFilterPolicy::new().allow_suffix("example.com");
        assert!(policy.check("HTTPS://ANCHOR.EXAMPLE.COM:443/sep6").is_ok());
    }

    #[test]
    fn test_policy_exact_allowlist() {
        let policy = UrlFilterPolicy::new().allow_exact("anchor.example.com");
        assert!(policy.check("https://anchor.example.com").is_ok());
        // Subdomain of allowed host should fail with exact allowlist.
        let err = policy.check("https://api.anchor.example.com").unwrap_err();
        assert!(err.message.contains("allowlist"));
    }

    #[test]
    fn test_policy_multiple_allowlist_entries() {
        let policy = UrlFilterPolicy::new()
            .allow_suffix("example.com")
            .allow_suffix("stellar.org");
        assert!(policy.check("https://anchor.example.com").is_ok());
        assert!(policy.check("https://api.stellar.org").is_ok());
        assert!(policy.check("https://other.io").is_err());
    }

    // ── normalize_and_validate ────────────────────────────────────────────

    #[test]
    fn test_normalize_and_validate_canonical_url() {
        let policy = UrlFilterPolicy::new().allow_suffix("example.com");
        let result = normalize_and_validate("HTTPS://Anchor.Example.COM:443/sep6", &policy);
        assert_eq!(result.unwrap(), "https://anchor.example.com/sep6");
    }

    #[test]
    fn test_normalize_and_validate_blocked_host_error() {
        let policy = UrlFilterPolicy::new()
            .allow_suffix("example.com")
            .block_exact("bad.example.com");
        let err = normalize_and_validate("https://bad.example.com/sep6", &policy).unwrap_err();
        assert!(err.message.contains("blocklist"));
    }

    #[test]
    fn test_normalize_and_validate_structural_error_propagated() {
        let policy = UrlFilterPolicy::new();
        // IP address — rejected by validate_anchor_domain.
        assert!(normalize_and_validate("https://192.168.1.1/sep6", &policy).is_err());
    }

    #[test]
    fn test_normalize_and_validate_strips_default_port_before_structural_check() {
        // https://example.com:443 → normalized to https://example.com → passes.
        let policy = UrlFilterPolicy::new();
        let result = normalize_and_validate("https://anchor.example.com:443", &policy);
        assert_eq!(result.unwrap(), "https://anchor.example.com");
    }

    #[test]
    fn test_normalize_and_validate_bare_trailing_slash_stripped() {
        let policy = UrlFilterPolicy::new();
        let result = normalize_and_validate("https://anchor.example.com/", &policy);
        assert_eq!(result.unwrap(), "https://anchor.example.com");
    }
}
