//! Domain validation utility for anchor domain input
//!
//! Validates anchor domain URLs before making requests to ensure:
//! - Proper URL format
//! - HTTPS-only connections (TLS enforcement)
//! - No embedded userinfo credentials
//! - Rejection of IP addresses (IPv4 and IPv6)
//! - Rejection of malformed or reserved hostnames
//! - Policy-based allow/deny rules for anchor domains

extern crate alloc;
use alloc::{string::String, vec::Vec};

use crate::errors::AnchorKitError;

// ---------------------------------------------------------------------------
// Domain policy rules
// ---------------------------------------------------------------------------

/// A single domain policy rule — either allow or deny a host pattern.
///
/// Pattern matching is suffix-based (e.g. `"example.com"` matches
/// `"anchor.example.com"`, `"api.example.com"`, and `"example.com"` itself).
/// An empty pattern matches nothing.
///
/// Rules are evaluated in declaration order; the first matching rule wins.
/// If no rule matches, the default is **allow** (open policy).
#[derive(Clone, Debug, PartialEq)]
pub struct DomainPolicyRule {
    /// Whether this rule allows or denies the matched host.
    pub action: PolicyAction,
    /// Hostname suffix pattern to match (case-insensitive ASCII).
    /// `"example.com"` matches `"example.com"` and `"sub.example.com"`.
    pub host_pattern: String,
}

/// Policy action for a matched domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PolicyAction {
    Allow,
    Deny,
}

/// A set of ordered domain policy rules applied after baseline URL validation.
///
/// Rules are evaluated in declaration order; the first match wins.
/// When no rule matches, the default action is **allow** (open policy),
/// unless `default_deny` is `true`, in which case unmatched hosts are rejected.
#[derive(Clone, Debug, Default)]
pub struct DomainPolicy {
    /// Ordered list of rules. First match wins.
    pub rules: Vec<DomainPolicyRule>,
    /// When `true`, unmatched hosts are denied. When `false` (default), unmatched hosts are allowed.
    pub default_deny: bool,
}

impl DomainPolicy {
    /// Create an empty policy that allows all hosts (open policy).
    pub fn allow_all() -> Self {
        DomainPolicy { rules: Vec::new(), default_deny: false }
    }

    /// Create a policy that denies all hosts not explicitly allowed.
    pub fn deny_all() -> Self {
        DomainPolicy { rules: Vec::new(), default_deny: true }
    }

    /// Add an allow rule for `host_pattern` and return `self` for chaining.
    pub fn with_allow(mut self, host_pattern: impl Into<String>) -> Self {
        self.rules.push(DomainPolicyRule {
            action: PolicyAction::Allow,
            host_pattern: host_pattern.into(),
        });
        self
    }

    /// Add a deny rule for `host_pattern` and return `self` for chaining.
    pub fn with_deny(mut self, host_pattern: impl Into<String>) -> Self {
        self.rules.push(DomainPolicyRule {
            action: PolicyAction::Deny,
            host_pattern: host_pattern.into(),
        });
        self
    }

    /// Evaluate the policy for `host` (the bare hostname, no port, no scheme).
    ///
    /// Returns `true` when the host is permitted, `false` when denied.
    pub fn permits(&self, host: &str) -> bool {
        let host_lower = host.to_ascii_lowercase();
        for rule in &self.rules {
            let pattern = rule.host_pattern.to_ascii_lowercase();
            if pattern.is_empty() {
                continue;
            }
            // Suffix match: host == pattern OR host ends with ".<pattern>"
            if host_lower == pattern || host_lower.ends_with(&alloc::format!(".{}", pattern)) {
                return rule.action == PolicyAction::Allow;
            }
        }
        // No rule matched — apply default
        !self.default_deny
    }
}

/// Validates an anchor domain URL.
///
/// Ensures the URL is safe to use as an anchor endpoint before making any
/// outbound HTTP requests. The following rules are enforced:
///
/// - Must be non-empty and between 10 and 2048 characters.
/// - Must use the `https://` scheme exactly (HTTP, FTP, WS, WSS, etc. are rejected).
/// - No embedded userinfo credentials (`user:pass@` or `user@` in the authority).
/// - Host must contain at least one dot (no bare hostnames like `localhost`).
/// - No consecutive dots, leading/trailing dots, or leading/trailing hyphens
///   in any DNS label.
/// - No IPv4 addresses (all-numeric labels).
/// - No IPv6 addresses (bracket notation `[...]` is rejected).
/// - No reserved or private IP ranges expressed as hostnames.
/// - Port (if present) must be in the range 1–65535.
/// - No control characters, whitespace, or RFC 3986 invalid characters
///   (`<`, `>`, `{`, `}`, `|`, `\`, `[`, `]`, `^`, `` ` ``).
/// - Unicode / IDN domains are rejected (ASCII only).
///
/// # Arguments
///
/// * `domain` - The full URL string to validate (e.g. `"https://anchor.example.com"`).
///
/// # Returns
///
/// `Ok(())` if the domain passes all checks.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::InvalidEndpointFormat`] if
/// any validation rule is violated.
///
/// # Examples
///
/// ```rust
/// use anchorkit::validate_anchor_domain;
///
/// // Valid HTTPS domain
/// assert!(validate_anchor_domain("https://anchor.example.com").is_ok());
///
/// // Valid with port and path
/// assert!(validate_anchor_domain("https://api.example.com:8080/sep6").is_ok());
///
/// // Rejected: HTTP
/// assert!(validate_anchor_domain("http://example.com").is_err());
///
/// // Rejected: no TLD
/// assert!(validate_anchor_domain("https://localhost").is_err());
///
/// // Rejected: IPv4
/// assert!(validate_anchor_domain("https://192.168.1.1").is_err());
///
/// // Rejected: userinfo credentials
/// assert!(validate_anchor_domain("https://user:pass@example.com").is_err());
///
/// // Rejected: IPv6
/// assert!(validate_anchor_domain("https://[::1]").is_err());
/// ```
pub fn validate_anchor_domain(domain: &str) -> Result<(), AnchorKitError> {
    // Reject any leading or trailing whitespace before any other check.
    // We do NOT trim — callers must supply a clean string.
    if domain != domain.trim() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Check for empty input
    if domain.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Check minimum length for valid HTTPS URL ("https://a.b" = 12 chars)
    if domain.len() < 12 {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Check maximum reasonable length
    if domain.len() > 2048 {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // --- Scheme check: must be exactly "https://" ---
    // Reject any other scheme, including http, ftp, ws, wss, file, mailto, etc.
    if !domain.starts_with("https://") {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Validate characters across the entire URL before any structural parsing.
    // This catches control chars, whitespace, and RFC 3986 forbidden characters
    // early, including bracket notation used for IPv6.
    validate_url_characters(domain)?;

    // Extract the authority (host[:port]) from after "https://"
    let after_scheme = &domain[8..]; // skip "https://"

    if after_scheme.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // The authority ends at the first '/', '?', or '#'.
    let authority = match after_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => &after_scheme[..pos],
        None => after_scheme,
    };

    if authority.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // --- Userinfo check ---
    // RFC 3986 §3.2.1: userinfo is the substring before '@' in the authority.
    // Any '@' in the authority means credentials are embedded — always reject.
    if authority.contains('@') {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Validate the host (and optional port) portion.
    validate_host(authority)?;

    Ok(())
}

/// Validates the host[:port] portion of a URL authority.
///
/// The authority must not contain userinfo (that is checked by the caller).
fn validate_host(host: &str) -> Result<(), AnchorKitError> {
    if host.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Spaces are not valid in a host.
    if host.contains(' ') {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Separate the optional port from the hostname.
    // Use rfind(':') so that a bare IPv6 address (already rejected by
    // validate_url_characters via '['/']') would not confuse port parsing.
    let domain_without_port = if let Some(colon_pos) = host.rfind(':') {
        let port_str = &host[colon_pos + 1..];

        if port_str.is_empty() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }

        // Port must be all ASCII digits.
        if !port_str.chars().all(|c| c.is_ascii_digit()) {
            return Err(AnchorKitError::invalid_endpoint_format());
        }

        // Port must be in the range 1–65535.
        match port_str.parse::<u32>() {
            Ok(p) if p >= 1 && p <= 65535 => {}
            _ => return Err(AnchorKitError::invalid_endpoint_format()),
        }

        &host[..colon_pos]
    } else {
        host
    };

    if domain_without_port.is_empty() {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Must contain at least one dot — rejects bare hostnames like "localhost".
    if !domain_without_port.contains('.') {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Reject consecutive dots.
    if domain_without_port.contains("..") {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Reject leading or trailing dots.
    if domain_without_port.starts_with('.') || domain_without_port.ends_with('.') {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    // Split into DNS labels and validate each one.
    let labels: Vec<&str> = domain_without_port.split('.').collect();

    // Reject IPv4-style addresses: every label is purely numeric (e.g. 192.168.1.1).
    if labels.iter().all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    for label in &labels {
        if label.is_empty() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }
        
        // Label must start and end with ASCII alphanumeric (reject Unicode/IDN)
        let first_char = label.chars().next().unwrap();
        let last_char = label.chars().last().unwrap();
        
        if !first_char.is_ascii_alphanumeric() || !last_char.is_ascii_alphanumeric() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }
        
        // Check for valid characters in label (ASCII only — reject Unicode/IDN)
        for c in label.chars() {
            if !c.is_ascii_alphanumeric() && c != '-' {
                return Err(AnchorKitError::invalid_endpoint_format());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Policy-aware validation
// ---------------------------------------------------------------------------

/// Extract the bare hostname (no port, no scheme, no path) from a URL that
/// has already passed [`validate_anchor_domain`].
///
/// Returns an empty string when the URL cannot be parsed (should not happen
/// after validation, but is safe).
fn extract_host(domain: &str) -> &str {
    // Strip "https://"
    let after_scheme = match domain.strip_prefix("https://") {
        Some(s) => s,
        None => return "",
    };
    // Authority ends at first '/', '?', '#'
    let authority = match after_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => &after_scheme[..pos],
        None => after_scheme,
    };
    // Strip port
    match authority.rfind(':') {
        Some(colon) => {
            let port_part = &authority[colon + 1..];
            // Only strip if it looks like a port (all digits)
            if !port_part.is_empty() && port_part.chars().all(|c| c.is_ascii_digit()) {
                &authority[..colon]
            } else {
                authority
            }
        }
        None => authority,
    }
}

/// Validate an anchor domain URL against baseline security rules **and** an
/// optional deployment [`DomainPolicy`].
///
/// This is the recommended entry-point for production deployments that need to
/// enforce an explicit allow/deny list of anchor hosts in addition to the
/// baseline checks performed by [`validate_anchor_domain`].
///
/// Validation order:
/// 1. All checks from [`validate_anchor_domain`] (HTTPS scheme, URL characters,
///    userinfo, host format, port range, IPv4/IPv6 rejection).
/// 2. Policy evaluation: the bare hostname is extracted and tested against
///    `policy.permits(host)`. A denied host produces
///    [`ErrorCode::InvalidEndpointFormat`] with a descriptive context string so
///    callers can log the specific reason.
///
/// When `policy` is `None` (or is the default open policy), this function
/// behaves identically to [`validate_anchor_domain`].
///
/// # Arguments
///
/// * `domain` — The full HTTPS URL to validate.
/// * `policy` — Optional [`DomainPolicy`] to enforce.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::InvalidEndpointFormat`]
/// when any baseline or policy check fails.
///
/// # Examples
///
/// ```rust
/// use anchorkit::domain_validator::{validate_anchor_domain_with_policy, DomainPolicy, PolicyAction};
///
/// // Open policy — behaves like validate_anchor_domain.
/// assert!(validate_anchor_domain_with_policy(
///     "https://anchor.example.com",
///     Some(&DomainPolicy::allow_all()),
/// ).is_ok());
///
/// // Deny list — specific host blocked.
/// let policy = DomainPolicy::allow_all().with_deny("evil.com");
/// assert!(validate_anchor_domain_with_policy("https://evil.com/sep6", Some(&policy)).is_err());
/// assert!(validate_anchor_domain_with_policy("https://good.example.com", Some(&policy)).is_ok());
///
/// // Allow list — only the named anchor is permitted.
/// let policy = DomainPolicy::deny_all().with_allow("anchor.example.com");
/// assert!(validate_anchor_domain_with_policy("https://anchor.example.com", Some(&policy)).is_ok());
/// assert!(validate_anchor_domain_with_policy("https://other.example.com", Some(&policy)).is_err());
/// ```
pub fn validate_anchor_domain_with_policy(
    domain: &str,
    policy: Option<&DomainPolicy>,
) -> Result<(), AnchorKitError> {
    // Step 1: all baseline security checks.
    validate_anchor_domain(domain)?;

    // Step 2: policy check (skip if no policy is provided).
    if let Some(pol) = policy {
        let host = extract_host(domain);
        if !pol.permits(host) {
            return Err(AnchorKitError::with_context(
                crate::errors::ErrorCode::InvalidEndpointFormat,
                "Anchor domain is blocked by deployment policy",
                host,
            ));
        }
    }

    Ok(())
}

/// Validates that every character in the URL is acceptable.
///
/// Rejects:
/// - Any ASCII control character (0x00–0x1F, 0x7F)
/// - ASCII whitespace (space, tab, CR, LF, etc.)
/// - Characters forbidden in URLs per RFC 3986 / security best practice:
///   `<`, `>`, `{`, `}`, `|`, `\`, `[`, `]`, `^`, `` ` ``
/// - Any non-ASCII character (Unicode / IDN hostnames are not supported)
fn validate_url_characters(url: &str) -> Result<(), AnchorKitError> {
    for c in url.chars() {
        // Reject non-ASCII (covers Unicode / IDN domains).
        if !c.is_ascii() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }

        // Reject ASCII control characters (includes \0, \t, \n, \r, DEL).
        if c.is_ascii_control() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }

        // Reject ASCII whitespace (space is not a control char but is invalid).
        if c == ' ' {
            return Err(AnchorKitError::invalid_endpoint_format());
        }

        // Reject characters that are invalid or dangerous in URLs.
        // '[' and ']' are included to block IPv6 bracket notation.
        if matches!(c, '<' | '>' | '{' | '}' | '|' | '\\' | '[' | ']' | '^' | '`') {
            return Err(AnchorKitError::invalid_endpoint_format());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    // ------------------------------------------------------------------
    // Valid domains
    // ------------------------------------------------------------------

    #[test]
    fn test_valid_domains() {
        assert!(validate_anchor_domain("https://example.com").is_ok());
        assert!(validate_anchor_domain("https://api.example.com").is_ok());
        assert!(validate_anchor_domain("https://sub.domain.example.com").is_ok());
        assert!(validate_anchor_domain("https://example.com/path").is_ok());
        assert!(validate_anchor_domain("https://example.com/path/to/resource").is_ok());
        assert!(validate_anchor_domain("https://example.com:8080").is_ok());
        assert!(validate_anchor_domain("https://example.com:443").is_ok());
        assert!(validate_anchor_domain("https://example.com?param=value").is_ok());
        assert!(validate_anchor_domain("https://example.com/path?param=value").is_ok());
        assert!(validate_anchor_domain("https://my-domain.com").is_ok());
        assert!(validate_anchor_domain("https://api-v2.example.com").is_ok());
    }

    // ------------------------------------------------------------------
    // Scheme enforcement: only https:// is accepted
    // ------------------------------------------------------------------

    #[test]
    fn test_https_only() {
        assert!(validate_anchor_domain("http://example.com").is_err());
        assert!(validate_anchor_domain("http://secure.example.com").is_err());
        assert!(validate_anchor_domain("ftp://example.com").is_err());
        assert!(validate_anchor_domain("ws://example.com").is_err());
        assert!(validate_anchor_domain("wss://example.com").is_err());
        assert!(validate_anchor_domain("file://example.com").is_err());
        assert!(validate_anchor_domain("mailto:example@example.com").is_err());
    }

    #[test]
    fn test_insecure_http_rejected() {
        // Plain HTTP must never be accepted for anchor endpoints.
        assert!(validate_anchor_domain("http://anchor.example.com").is_err());
        assert!(validate_anchor_domain("http://api.stellar.org").is_err());
        assert!(validate_anchor_domain("http://example.com:80").is_err());
        assert!(validate_anchor_domain("http://example.com/sep6").is_err());
    }

    #[test]
    fn test_protocol_variations() {
        assert!(validate_anchor_domain("https://example.com").is_ok());
        assert!(validate_anchor_domain("http://example.com").is_err());
        assert!(validate_anchor_domain("ftp://example.com").is_err());
        assert!(validate_anchor_domain("ws://example.com").is_err());
        assert!(validate_anchor_domain("wss://example.com").is_err());
        assert!(validate_anchor_domain("file://example.com").is_err());
        assert!(validate_anchor_domain("mailto:example@example.com").is_err());
    }

    // ------------------------------------------------------------------
    // Userinfo / embedded credentials
    // ------------------------------------------------------------------

    #[test]
    fn test_userinfo_credentials_rejected() {
        // user:pass@host form
        assert!(validate_anchor_domain("https://user:pass@example.com").is_err());
        assert!(validate_anchor_domain("https://admin:secret@anchor.example.com").is_err());
        assert!(validate_anchor_domain("https://root:hunter2@api.example.com:8080").is_err());

        // user@host form (no password)
        assert!(validate_anchor_domain("https://user@example.com").is_err());
        assert!(validate_anchor_domain("https://alice@anchor.example.com").is_err());

        // @ anywhere in authority
        assert!(validate_anchor_domain("https://@example.com").is_err());
        assert!(validate_anchor_domain("https://example.com@evil.com").is_err());
    }

    // ------------------------------------------------------------------
    // IPv4 and IPv6 addresses
    // ------------------------------------------------------------------

    #[test]
    fn test_ip_address_inputs() {
        // IPv4 — all-numeric labels
        assert!(validate_anchor_domain("https://192.168.1.1").is_err());
        assert!(validate_anchor_domain("https://10.0.0.1").is_err());
        assert!(validate_anchor_domain("https://127.0.0.1").is_err());
        assert!(validate_anchor_domain("https://0.0.0.0").is_err());
        assert!(validate_anchor_domain("https://255.255.255.255").is_err());

        // IPv6 — bracket notation
        assert!(validate_anchor_domain("https://[::1]").is_err());
        assert!(validate_anchor_domain("https://[2001:db8::1]").is_err());
        assert!(validate_anchor_domain("https://[::ffff:192.0.2.1]").is_err());
        assert!(validate_anchor_domain("https://[fe80::1%25eth0]").is_err());
    }

    #[test]
    fn test_reserved_ip_ranges_rejected() {
        // Private / loopback / link-local ranges expressed as IPv4 dotted-decimal
        assert!(validate_anchor_domain("https://10.0.0.1").is_err());
        assert!(validate_anchor_domain("https://172.16.0.1").is_err());
        assert!(validate_anchor_domain("https://192.168.0.1").is_err());
        assert!(validate_anchor_domain("https://127.0.0.1").is_err());
        assert!(validate_anchor_domain("https://169.254.0.1").is_err());
    }

    // ------------------------------------------------------------------
    // Internationalized / Unicode domain names
    // ------------------------------------------------------------------

    #[test]
    fn test_unicode_idn_domains_rejected() {
        assert!(validate_anchor_domain("https://münchen.de").is_err());
        assert!(validate_anchor_domain("https://例え.jp").is_err());
        assert!(validate_anchor_domain("https://россия.рф").is_err());
        assert!(validate_anchor_domain("https://example.测试").is_err());
        // Punycode-encoded IDN is ASCII and therefore accepted by character
        // validation, but the label "xn--mnchen-3ya" contains only valid chars.
        assert!(validate_anchor_domain("https://xn--mnchen-3ya.de").is_ok());
    }

    // ------------------------------------------------------------------
    // Malformed hostnames
    // ------------------------------------------------------------------

    #[test]
    fn test_malformed_domains() {
        assert!(validate_anchor_domain("").is_err());
        assert!(validate_anchor_domain("   ").is_err());
        assert!(validate_anchor_domain("example.com").is_err());
        assert!(validate_anchor_domain("www.example.com").is_err());
        assert!(validate_anchor_domain("https://").is_err());
        assert!(validate_anchor_domain("https://.example.com").is_err());
        assert!(validate_anchor_domain("https://example.com.").is_err());
        assert!(validate_anchor_domain("https://example..com").is_err());
        assert!(validate_anchor_domain("https://localhost").is_err());
        assert!(validate_anchor_domain("https://example").is_err());
        assert!(validate_anchor_domain("https://example .com").is_err());
        assert!(validate_anchor_domain("https://exam ple.com").is_err());
        assert!(validate_anchor_domain("https://example$.com").is_err());
        assert!(validate_anchor_domain("https://a").is_err());
        assert!(validate_anchor_domain("https://a.").is_err());
    }

    #[test]
    fn test_malformed_hostnames_rejected() {
        // Labels starting or ending with a hyphen
        assert!(validate_anchor_domain("https://-example.com").is_err());
        assert!(validate_anchor_domain("https://example-.com").is_err());
        assert!(validate_anchor_domain("https://sub.-example.com").is_err());
        assert!(validate_anchor_domain("https://sub.example-.com").is_err());

        // Empty labels (consecutive dots or leading/trailing dot)
        assert!(validate_anchor_domain("https://.example.com").is_err());
        assert!(validate_anchor_domain("https://example.com.").is_err());
        assert!(validate_anchor_domain("https://example..com").is_err());

        // Bare hostname without TLD
        assert!(validate_anchor_domain("https://localhost").is_err());
        assert!(validate_anchor_domain("https://intranet").is_err());

        // Invalid characters in labels
        assert!(validate_anchor_domain("https://example$.com").is_err());
        assert!(validate_anchor_domain("https://exam_ple.com").is_err());
        assert!(validate_anchor_domain("https://exam!ple.com").is_err());
    }

    // ------------------------------------------------------------------
    // Port validation
    // ------------------------------------------------------------------

    #[test]
    fn test_port_validation() {
        assert!(validate_anchor_domain("https://example.com:1").is_ok());
        assert!(validate_anchor_domain("https://example.com:80").is_ok());
        assert!(validate_anchor_domain("https://example.com:443").is_ok());
        assert!(validate_anchor_domain("https://example.com:8080").is_ok());
        assert!(validate_anchor_domain("https://example.com:65535").is_ok());

        assert!(validate_anchor_domain("https://example.com:0").is_err());
        assert!(validate_anchor_domain("https://example.com:65536").is_err());
        assert!(validate_anchor_domain("https://example.com:99999").is_err());
        assert!(validate_anchor_domain("https://example.com:").is_err());
        assert!(validate_anchor_domain("https://example.com:abc").is_err());
    }

    #[test]
    fn test_port_edge_cases() {
        assert!(validate_anchor_domain("https://example.com:1").is_ok());
        assert!(validate_anchor_domain("https://example.com:65535").is_ok());
        assert!(validate_anchor_domain("https://example.com:0").is_err());
        assert!(validate_anchor_domain("https://example.com:65536").is_err());
        assert!(validate_anchor_domain("https://example.com:99999").is_err());
        assert!(validate_anchor_domain("https://example.com:8080/path").is_ok());
        assert!(validate_anchor_domain("https://example.com:8080/path?query=value").is_ok());
    }

    // ------------------------------------------------------------------
    // Length limits
    // ------------------------------------------------------------------

    #[test]
    fn test_length_limits() {
        let long_domain = format!("https://{}.com", "a".repeat(2048));
        assert!(validate_anchor_domain(&long_domain).is_err());

        let max_domain = format!("https://{}.com", "a".repeat(2000));
        assert!(validate_anchor_domain(&max_domain).is_ok());
    }

    // NOTE: duplicate test name existed in this module in two different
    // blocks. Keeping one authoritative boundary test and renaming the other
    // to avoid a compile-time duplicate definition error.
    #[test]
    fn test_length_boundaries_case_1() {
        // "https://" (8) + label + ".com" (4) = 12 + label_len
        // Max total = 2048, so max label_len = 2036.
        let max_valid_domain = format!("https://{}.com", "a".repeat(2036));
        assert_eq!(max_valid_domain.len(), 2048);
        assert!(validate_anchor_domain(&max_valid_domain).is_ok());

        let too_long_domain = format!("https://{}.com", "a".repeat(2037));
        assert_eq!(too_long_domain.len(), 2049);
        assert!(validate_anchor_domain(&too_long_domain).is_err());

        assert!(validate_anchor_domain("https://a.b").is_ok());
        assert!(validate_anchor_domain("https://ab.cd").is_ok());
    }


    // ------------------------------------------------------------------
    // Control characters and whitespace
    // ------------------------------------------------------------------

    #[test]
    fn test_control_characters() {
        assert!(validate_anchor_domain("https://example.com\n").is_err());
        assert!(validate_anchor_domain("https://example.com\r").is_err());
        assert!(validate_anchor_domain("https://example.com\t").is_err());
        assert!(validate_anchor_domain("https://\0example.com").is_err());
    }

    #[test]
    fn test_whitespace_variations() {
        assert!(validate_anchor_domain(" https://example.com").is_err());
        assert!(validate_anchor_domain("https://example.com ").is_err());
        assert!(validate_anchor_domain("  https://example.com  ").is_err());
        assert!(validate_anchor_domain("https://example .com").is_err());
        assert!(validate_anchor_domain("https://exam ple.com").is_err());
    }

    // ------------------------------------------------------------------
    // Invalid URL characters
    // ------------------------------------------------------------------

    #[test]
    fn test_special_characters_in_path() {
        assert!(validate_anchor_domain("https://example.com/path-with-dash").is_ok());
        assert!(validate_anchor_domain("https://example.com/path_with_underscore").is_ok());
        assert!(validate_anchor_domain("https://example.com/path.with.dot").is_ok());
        assert!(validate_anchor_domain("https://example.com/path~tilde").is_ok());
        assert!(validate_anchor_domain("https://example.com/path%20encoded").is_ok());

        assert!(validate_anchor_domain("https://example.com/path<invalid>").is_err());
        assert!(validate_anchor_domain("https://example.com/path{invalid}").is_err());
        assert!(validate_anchor_domain("https://example.com/path|pipe").is_err());
        assert!(validate_anchor_domain("https://example.com/path\\backslash").is_err());
    }

    // ------------------------------------------------------------------
    // Edge cases
    // ------------------------------------------------------------------

    #[test]
    fn test_edge_cases() {
        assert!(validate_anchor_domain("https://a.b").is_ok());
        assert!(validate_anchor_domain("https://a.b.c.d.example.com").is_ok());
        assert!(validate_anchor_domain("https://api2.example.com").is_ok());
        assert!(validate_anchor_domain("https://123.example.com").is_ok());
        assert!(validate_anchor_domain("https://my-api.example.com").is_ok());
        assert!(validate_anchor_domain("https://-example.com").is_err());
        assert!(validate_anchor_domain("https://example-.com").is_err());
    }

    #[test]
    fn test_double_slashes_in_path() {
        assert!(validate_anchor_domain("https://example.com//path").is_ok());
        assert!(validate_anchor_domain("https://example.com/path//resource").is_ok());
    }

    #[test]
    fn test_trailing_slashes() {
        assert!(validate_anchor_domain("https://example.com/").is_ok());
        assert!(validate_anchor_domain("https://example.com/path/").is_ok());
        assert!(validate_anchor_domain("https://example.com//").is_ok());
    }

    #[test]
    fn test_length_boundaries() {
        // Domain exactly at 2048-character limit (should pass)
        // "https://" (8) + "a"*2036 + ".com" (4) = 2048 chars
        let max_valid_domain = format!("https://{}.com", "a".repeat(2036));
        assert!(validate_anchor_domain(&max_valid_domain).is_ok());
        
        // Domain exceeding 2048-character limit (should fail)
        let too_long_domain = format!("https://{}.com", "a".repeat(2037));
        assert!(validate_anchor_domain(&too_long_domain).is_err());
        
        // Very short valid domains
        assert!(validate_anchor_domain("https://a.b").is_ok());
        assert!(validate_anchor_domain("https://ab.cd").is_ok());
    }

    #[test]
    fn test_query_parameters_and_fragments() {
        assert!(validate_anchor_domain("https://example.com?param=value").is_ok());
        assert!(validate_anchor_domain("https://example.com?param1=value1&param2=value2").is_ok());
        assert!(validate_anchor_domain("https://example.com#section").is_ok());
        assert!(validate_anchor_domain("https://example.com/path#section").is_ok());
        assert!(validate_anchor_domain("https://example.com?param=value#section").is_ok());
    }

    #[test]
    fn test_domain_label_edge_cases() {
        assert!(validate_anchor_domain("https://a-b-c.example.com").is_ok());
        assert!(validate_anchor_domain("https://123-456.example.com").is_ok());
        assert!(validate_anchor_domain("https://a1b2c3.example.com").is_ok());
        assert!(validate_anchor_domain("https://-abc.example.com").is_err());
        assert!(validate_anchor_domain("https://abc-.example.com").is_err());
        // Double hyphens in the middle of a label are allowed (e.g. punycode xn--)
        assert!(validate_anchor_domain("https://a--b.example.com").is_ok());
        assert!(validate_anchor_domain("https://.example.com").is_err());
        assert!(validate_anchor_domain("https://example..com").is_err());
    }

    // ------------------------------------------------------------------
    // Policy validation tests
    // ------------------------------------------------------------------

    #[test]
    fn test_policy_open_allows_all_valid_domains() {
        let policy = DomainPolicy::allow_all();
        assert!(validate_anchor_domain_with_policy("https://anchor.example.com", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://api.other.org", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://anywhere.net", Some(&policy)).is_ok());
    }

    #[test]
    fn test_policy_open_still_rejects_unsafe_domains() {
        let policy = DomainPolicy::allow_all();
        // Baseline rules still apply even with an open policy
        assert!(validate_anchor_domain_with_policy("http://example.com", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://192.168.1.1", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://user:pass@example.com", Some(&policy)).is_err());
    }

    #[test]
    fn test_policy_deny_list_blocks_specific_host() {
        let policy = DomainPolicy::allow_all().with_deny("evil.com");
        assert!(validate_anchor_domain_with_policy("https://evil.com/sep6", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://sub.evil.com", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://good.example.com", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://notevil.com", Some(&policy)).is_ok());
    }

    #[test]
    fn test_policy_deny_list_with_port_and_path() {
        let policy = DomainPolicy::allow_all().with_deny("blocked.com");
        assert!(validate_anchor_domain_with_policy("https://blocked.com:8443/api", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://blocked.com?q=1", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://notblocked.com:8443", Some(&policy)).is_ok());
    }

    #[test]
    fn test_policy_allow_list_permits_only_named_anchor() {
        let policy = DomainPolicy::deny_all().with_allow("anchor.example.com");
        assert!(validate_anchor_domain_with_policy("https://anchor.example.com", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://sub.anchor.example.com", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://other.example.com", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://example.com", Some(&policy)).is_err());
    }

    #[test]
    fn test_policy_deny_all_rejects_unmatched() {
        let policy = DomainPolicy::deny_all();
        assert!(validate_anchor_domain_with_policy("https://any.example.com", Some(&policy)).is_err());
    }

    #[test]
    fn test_policy_first_rule_wins() {
        // Allow rule before deny rule — allow wins for the matched host
        let policy = DomainPolicy::allow_all()
            .with_allow("good.example.com")
            .with_deny("example.com");
        // "good.example.com" matches the allow rule first
        assert!(validate_anchor_domain_with_policy("https://good.example.com", Some(&policy)).is_ok());
        // "other.example.com" hits the deny rule
        assert!(validate_anchor_domain_with_policy("https://other.example.com", Some(&policy)).is_err());
    }

    #[test]
    fn test_policy_none_behaves_like_validate_anchor_domain() {
        assert!(validate_anchor_domain_with_policy("https://anchor.example.com", None).is_ok());
        assert!(validate_anchor_domain_with_policy("http://anchor.example.com", None).is_err());
    }

    #[test]
    fn test_policy_case_insensitive_matching() {
        let policy = DomainPolicy::allow_all().with_deny("Evil.COM");
        assert!(validate_anchor_domain_with_policy("https://evil.com", Some(&policy)).is_err());
        assert!(validate_anchor_domain_with_policy("https://EVIL.COM", Some(&policy)).is_err());
    }

    #[test]
    fn test_policy_blocked_error_has_host_in_context() {
        let policy = DomainPolicy::allow_all().with_deny("blocked.com");
        let err = validate_anchor_domain_with_policy("https://blocked.com", Some(&policy)).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidEndpointFormat);
        assert!(err.context.as_deref().unwrap_or("").contains("blocked.com"),
            "context should contain the blocked host");
        assert!(err.message.contains("policy"),
            "message should mention policy");
    }

    #[test]
    fn test_policy_suffix_match_does_not_match_partial_label() {
        // "evil.com" must not match "notevil.com" (suffix match requires a dot boundary)
        let policy = DomainPolicy::allow_all().with_deny("evil.com");
        assert!(validate_anchor_domain_with_policy("https://notevil.com", Some(&policy)).is_ok());
        assert!(validate_anchor_domain_with_policy("https://evilcorp.com", Some(&policy)).is_ok());
    }

    #[test]
    fn test_extract_host_strips_scheme_port_path() {
        assert_eq!(extract_host("https://example.com"), "example.com");
        assert_eq!(extract_host("https://example.com:8080"), "example.com");
        assert_eq!(extract_host("https://example.com:443/sep6"), "example.com");
        assert_eq!(extract_host("https://api.example.com?q=1"), "api.example.com");
        assert_eq!(extract_host("https://sub.example.com:9000/path?x=1#frag"), "sub.example.com");
    }
}
