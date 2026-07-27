//! Minimal SEP-10 JWT verification (JWS compact, Ed25519 / `EdDSA`) for Soroban.
//!
//! Verifies the anchor-signed token using a 32-byte Ed25519 public key stored on-chain.
//! Payload must include integer `exp` (Unix seconds) and string `sub` (Stellar strkey of the client).

extern crate alloc;

use alloc::vec::Vec;
use core::convert::TryInto;
use soroban_sdk::{Bytes, BytesN, Env, String};

/// Default maximum JWT character length. Can be overridden via contract storage key "JWTMAXLEN".
pub const MAX_JWT_LEN: u32 = 2048;

/// Storage key used by the admin to configure a custom JWT max length.
pub const JWT_MAX_LEN_KEY: &[u8] = b"JWTMAXLEN";

/// Maximum allowed token lifetime in seconds (24 hours).
pub const MAX_JWT_LIFETIME: u64 = 86_400;

/// Default clock skew tolerance in seconds.
pub const DEFAULT_CLOCK_SKEW: u64 = 60;

/// Maximum allowed JTI byte length (prevents bloated keys in storage).
pub const JTI_MAX_BYTES: usize = 256;

/// Extra ledger buffer added to a JTI TTL to absorb clock skew (~10 minutes).
pub const JTI_TTL_BUFFER_LEDGERS: u32 = 120;

trait Ed25519VerifyResult {
    fn into_verify_result(self) -> Result<(), ()>;
}

impl Ed25519VerifyResult for () {
    fn into_verify_result(self) -> Result<(), ()> {
        Ok(())
    }
}

impl<T, E> Ed25519VerifyResult for Result<T, E> {
    fn into_verify_result(self) -> Result<(), ()> {
        self.map(|_| ()).map_err(|_| ())
    }
}

fn check_ed25519_verify<R: Ed25519VerifyResult>(result: R) -> Result<(), ()> {
    result.into_verify_result()
}

fn decode_base64url_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// Base64url decode (no padding required).
///
/// Decodes a base64url-encoded byte slice (RFC 4648 §5) without requiring
/// `=` padding characters. Stops at the first `=` if present.
///
/// # Arguments
///
/// * `input` - Base64url-encoded bytes.
///
/// # Returns
///
/// `Ok(Vec<u8>)` on success, `Err(())` if any character is not valid base64url.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep10_jwt::base64url_decode;
///
/// let decoded = base64url_decode(b"SGVsbG8").unwrap();
/// assert_eq!(decoded, b"Hello");
/// ```
pub fn base64url_decode(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &ch in input {
        if ch == b'=' {
            break;
        }
        let val = decode_base64url_char(ch).ok_or(())?;
        buffer = (buffer << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Parse `"exp": <digits>` (first occurrence).
fn parse_json_exp(payload: &[u8]) -> Result<u64, ()> {
    let key = b"\"exp\":";
    let pos = find_bytes(payload, key).ok_or(())?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut n: u64 = 0;
    let mut any = false;
    while i < payload.len() && payload[i].is_ascii_digit() {
        any = true;
        let d = (payload[i] - b'0') as u64;
        n = n
            .checked_mul(10)
            .and_then(|x| x.checked_add(d))
            .ok_or(())?;
        i += 1;
    }
    if !any {
        return Err(());
    }
    Ok(n)
}

/// Parse `"nbf": <digits>` (first occurrence). Returns `None` if claim is absent.
fn parse_json_nbf(payload: &[u8]) -> Option<u64> {
    let key = b"\"nbf\":";
    let pos = find_bytes(payload, key)?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut n: u64 = 0;
    let mut any = false;
    while i < payload.len() && payload[i].is_ascii_digit() {
        any = true;
        let d = (payload[i] - b'0') as u64;
        n = n.checked_mul(10).and_then(|x| x.checked_add(d))?;
        i += 1;
    }
    if !any { None } else { Some(n) }
}

/// Parse `"iat": <digits>` (first occurrence). Returns `None` if claim is absent.
fn parse_json_iat(payload: &[u8]) -> Option<u64> {
    let key = b"\"iat\":";
    let pos = find_bytes(payload, key)?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut n: u64 = 0;
    let mut any = false;
    while i < payload.len() && payload[i].is_ascii_digit() {
        any = true;
        let d = (payload[i] - b'0') as u64;
        n = n.checked_mul(10).and_then(|x| x.checked_add(d))?;
        i += 1;
    }
    if !any { None } else { Some(n) }
}

/// Parse `"jti":"..."` string value (first occurrence). Returns `None` if absent.
fn parse_json_jti(payload: &[u8]) -> Option<Vec<u8>> {
    let key = b"\"jti\":";
    let pos = find_bytes(payload, key)?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= payload.len() || payload[i] != b'"' {
        return None;
    }
    i += 1;
    let start = i;
    while i < payload.len() {
        if payload[i] == b'"' {
            return Some(payload[start..i].to_vec());
        }
        i += 1;
    }
    None
}

/// Parse first `"iss":"..."` string value. Returns `None` if absent.
fn parse_json_iss(payload: &[u8]) -> Option<Vec<u8>> {
    let key = b"\"iss\":";
    let pos = find_bytes(payload, key)?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= payload.len() || payload[i] != b'"' {
        return None;
    }
    i += 1;
    let start = i;
    while i < payload.len() {
        if payload[i] == b'"' {
            return Some(payload[start..i].to_vec());
        }
        i += 1;
    }
    None
}

/// Parse first `"sub":"..."` string value (no escape sequences inside value).
fn parse_json_sub(env: &Env, payload: &[u8]) -> Result<String, ()> {
    let key = b"\"sub\":";
    let pos = find_bytes(payload, key).ok_or(())?;
    let mut i = pos + key.len();
    while i < payload.len() && payload[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= payload.len() || payload[i] != b'"' {
        return Err(());
    }
    i += 1;
    let start = i;
    while i < payload.len() {
        if payload[i] == b'"' {
            let sub = &payload[start..i];
            return Ok(String::from_bytes(env, sub));
        }
        i += 1;
    }
    Err(())
}

/// Verify a SEP-10-style JWT (JWS compact, EdDSA / Ed25519).
///
/// Performs the following checks in order:
/// 1. Token length is within the configured maximum (default [`MAX_JWT_LEN`]).
/// 2. Token has exactly two `.` separators (three parts).
/// 3. Header contains `"EdDSA"`.
/// 4. Signature is 64 bytes and passes Ed25519 verification against `anchor_public_key`.
/// 5. `exp` claim is present and in the future.
/// 6. `nbf` claim (if present) is not in the future.
/// 7. `jti` claim (if present) has not been seen before (replay protection).
/// 8. `sub` claim is present and, if `expected_sub` is `Some`, matches it.
///
/// The maximum accepted token length defaults to [`MAX_JWT_LEN`] (2048) but can
/// be overridden by storing a `u32` under the `"JWTMAXLEN"` instance storage key.
///
/// # Arguments
///
/// * `env` - The Soroban execution environment (used for crypto, ledger time, and storage).
/// * `token` - The compact JWS token string (`header.payload.signature`).
/// * `anchor_public_key` - The 32-byte Ed25519 public key of the signing anchor.
/// * `expected_sub` - When `Some`, the `sub` claim must equal this value.
///   When `None`, the `sub` claim is parsed but not compared.
///
/// # Returns
///
/// `Ok(())` if all checks pass.
///
/// # Errors
///
/// Returns `Err(())` if any check fails (invalid signature, expired token,
/// future `nbf`, replayed `jti`, mismatched `sub`, or malformed token).
///
/// # Examples
///
/// ```rust,no_run
/// # use soroban_sdk::{Env, Bytes, String};
/// # let env = Env::default();
/// # let anchor_public_key = Bytes::from_slice(&env, &[0u8; 32]);
/// # let token = String::from_str(&env, "header.payload.sig");
/// use anchorkit::sep10_jwt::verify_sep10_jwt;
///
/// Clock skew tolerance (seconds) is read from the `"JWTSKEW"` instance key; defaults to
/// [`DEFAULT_CLOCK_SKEW`] (60 s). A token whose `exp` is within the skew window of `now` is
/// still accepted. The `iat` claim is required; if `exp - iat` exceeds
/// [`MAX_JWT_LIFETIME`] (86 400 s) the token is rejected.
///
/// When `expected_sub` is [`None`], the token must still contain a parseable `sub` claim, but it
/// is not compared to a caller-supplied address (see contract `verify_sep10_token`).
pub fn verify_sep10_jwt(
    env: &Env,
    token: &String,
    anchor_public_key: &Bytes,
    expected_sub: Option<&String>,
) -> Result<(), ()> {
    if anchor_public_key.len() != 32 {
        return Err(());
    }

    // Issue #64: use admin-configured max length if set, else fall back to default
    let max_len: u32 = env
        .storage()
        .instance()
        .get::<_, u32>(&soroban_sdk::symbol_short!("JWTMAXLEN"))
        .unwrap_or(MAX_JWT_LEN);

    let n = token.len();
    if n == 0 || n > max_len {
        return Err(());
    }
    let n_usize = n as usize;

    // Allocate a buffer large enough for the configured max
    let mut buf: Vec<u8> = alloc::vec![0u8; max_len as usize];
    token.copy_into_slice(&mut buf[..n_usize]);

    let mut dots: [usize; 2] = [0; 2];
    let mut dot_count = 0usize;
    for i in 0..n_usize {
        if buf[i] == b'.' {
            if dot_count < 2 {
                dots[dot_count] = i;
                dot_count += 1;
            } else {
                return Err(());
            }
        }
    }
    if dot_count != 2 {
        return Err(());
    }

    let d0 = dots[0];
    let d1 = dots[1];
    if d0 == 0 || d1 <= d0 + 1 || d1 + 1 >= n_usize {
        return Err(());
    }

    let header_b64 = &buf[..d0];
    let payload_b64 = &buf[d0 + 1..d1];
    let sig_b64 = &buf[d1 + 1..n_usize];

    let header_dec = base64url_decode(header_b64).map_err(|_| ())?;
    if !contains_subslice(&header_dec, b"EdDSA") {
        return Err(());
    }
    // Reject tokens that claim a non-EdDSA algorithm alongside EdDSA (e.g. "alg":"RS256")
    // by requiring the header to NOT contain any of the common non-EdDSA algorithm names.
    for forbidden in &[b"RS256" as &[u8], b"RS384", b"RS512", b"HS256", b"HS384", b"HS512", b"ES256", b"ES384", b"ES512", b"none"] {
        if contains_subslice(&header_dec, forbidden) {
            return Err(());
        }
    }

    let sig_dec = base64url_decode(sig_b64).map_err(|_| ())?;
    if sig_dec.len() != 64 {
        return Err(());
    }

    let signing_input = Bytes::from_slice(env, &buf[..d1]);
    let sig_bytes = Bytes::from_slice(env, sig_dec.as_slice());

    let pk: BytesN<32> = anchor_public_key.clone().try_into().map_err(|_| ())?;
    let sig: BytesN<64> = sig_bytes.clone().try_into().map_err(|_| ())?;
    check_ed25519_verify(env.crypto().ed25519_verify(&pk, &signing_input, &sig))?;

    let payload_dec = base64url_decode(payload_b64).map_err(|_| ())?;
    let exp = parse_json_exp(&payload_dec)?;
    let now = env.ledger().timestamp();

    // Read configurable clock skew tolerance (JWTSKEW), default 60 s
    let skew: u64 = env
        .storage()
        .instance()
        .get::<_, u64>(&soroban_sdk::symbol_short!("JWTSKEW"))
        .unwrap_or(DEFAULT_CLOCK_SKEW);

    // Token is expired if exp + skew <= now
    if exp.saturating_add(skew) <= now {
        return Err(());
    }

    // Enforce maximum token lifetime: exp - iat must not exceed 24 hours.
    let iat = parse_json_iat(&payload_dec).ok_or(())?;
    if iat == 0 {
        return Err(());
    }
    // iat must not be in the future — a token cannot claim to have been issued
    // after the current ledger time (allow the same clock-skew tolerance).
    if iat > now.saturating_add(skew) {
        return Err(());
    }
    if exp.saturating_sub(iat) > MAX_JWT_LIFETIME {
        return Err(());
    }

    // Issue #61: reject tokens whose nbf is in the future
    if let Some(nbf) = parse_json_nbf(&payload_dec) {
        if nbf > now {
            return Err(());
        }
    }

    // Issue #63 / #550: persistent JTI replay protection across ledger boundaries.
    // Use persistent storage keyed by SHA-256("jti:" || jti_bytes) so the JTI
    // is remembered between separate contract invocations. TTL is derived from
    // the token's remaining lifetime plus a 10-minute buffer for clock skew.
    if let Some(jti_bytes) = parse_json_jti(&payload_dec) {
        if jti_bytes.len() > JTI_MAX_BYTES {
            return Err(());
        }
        // Build storage key: SHA-256(len_prefix("jti:") || len_prefix(jti_bytes))
        let mut key_input = Bytes::new(env);
        for &b in b"jti:" {
            key_input.push_back(b);
        }
        for &b in jti_bytes.as_slice() {
            key_input.push_back(b);
        }
        let jti_key = env.crypto().sha256(&key_input);
        if env.storage().persistent().has(&jti_key) {
            return Err(());
        }
        // TTL in ledgers: remaining token lifetime / 5 s/ledger + 120 buffer ledgers.
        let remaining_secs = exp.saturating_sub(now);
        let ttl_ledgers = ((remaining_secs / 5) as u32)
            .saturating_add(JTI_TTL_BUFFER_LEDGERS)
            .max(1);
        env.storage().persistent().set(&jti_key, &true);
        env.storage().persistent().extend_ttl(&jti_key, ttl_ledgers, ttl_ledgers);
    }

    // iss claim: must be present, non-empty, and must not be whitespace-only
    // or contain ASCII control characters. Normalize by trimming before the
    // emptiness check so `"iss":" "` is correctly rejected.
    let iss_bytes = match parse_json_iss(&payload_dec) {
        Some(b) if !b.is_empty() => b,
        _ => return Err(()),
    };
    // Trim leading/trailing whitespace and reject if nothing remains.
    let iss_trimmed_len = iss_bytes
        .iter()
        .rev()
        .skip_while(|b| b.is_ascii_whitespace())
        .count();
    let iss_leading = iss_bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    if iss_trimmed_len == 0 || (iss_leading >= iss_bytes.len()) {
        return Err(());
    }
    // Reject any control characters (0x00–0x1F, 0x7F) in the issuer.
    if iss_bytes.iter().any(|&b| b < 0x20 || b == 0x7F) {
        return Err(());
    }

    let sub = parse_json_sub(env, &payload_dec)?;
    // sub must be non-empty — an empty subject offers no identity binding.
    if sub.len() == 0 {
        return Err(());
    }

    if let Some(expected) = expected_sub {
        if sub != *expected {
            return Err(());
        }
    }

    Ok(())
}

/// Verify a SEP-10 JWT and additionally assert that the `iss` claim matches
/// `expected_issuer` (case-sensitive, after whitespace trimming on both sides).
///
/// This is a strict wrapper around [`verify_sep10_jwt`] for call sites where
/// the anchor identity is known ahead of time and must be explicitly validated
/// rather than merely present. All other checks from [`verify_sep10_jwt`]
/// apply unchanged.
///
/// # Arguments
///
/// * `env`              - Soroban execution environment.
/// * `token`            - Compact JWS token string.
/// * `anchor_public_key`- 32-byte Ed25519 public key.
/// * `expected_sub`     - Optional expected `sub` claim value.
/// * `expected_issuer`  - The issuer string that the `iss` claim must match
///                        (trimmed, case-sensitive comparison).
///
/// # Returns
///
/// `Ok(())` when all checks pass, `Err(())` otherwise.
pub fn verify_sep10_jwt_with_issuer(
    env: &Env,
    token: &String,
    anchor_public_key: &Bytes,
    expected_sub: Option<&String>,
    expected_issuer: &str,
) -> Result<(), ()> {
    if anchor_public_key.len() != 32 {
        return Err(());
    }

    // Reject an empty expected issuer immediately — a caller passing "" would
    // trivially accept any token and indicates a programming error.
    let expected_issuer_trimmed = expected_issuer.trim();
    if expected_issuer_trimmed.is_empty() {
        return Err(());
    }

    // Run the full base verification first (sig, exp, iat, nbf, jti, sub, iss).
    verify_sep10_jwt(env, token, anchor_public_key, expected_sub)?;

    // Re-decode the payload to extract iss for the explicit comparison.
    let max_len: u32 = env
        .storage()
        .instance()
        .get::<_, u32>(&soroban_sdk::symbol_short!("JWTMAXLEN"))
        .unwrap_or(MAX_JWT_LEN);

    let n = token.len() as usize;
    let mut buf: Vec<u8> = alloc::vec![0u8; max_len as usize];
    token.copy_into_slice(&mut buf[..n]);

    let mut dots = [0usize; 2];
    let mut dot_count = 0usize;
    for i in 0..n {
        if buf[i] == b'.' {
            if dot_count < 2 {
                dots[dot_count] = i;
                dot_count += 1;
            }
        }
    }
    if dot_count != 2 {
        return Err(());
    }

    let payload_b64 = &buf[dots[0] + 1..dots[1]];
    let payload_dec = base64url_decode(payload_b64).map_err(|_| ())?;

    let iss_bytes = match parse_json_iss(&payload_dec) {
        Some(b) if !b.is_empty() => b,
        _ => return Err(()),
    };

    // Trim and compare (case-sensitive).
    let iss_str = core::str::from_utf8(&iss_bytes).map_err(|_| ())?;
    if iss_str.trim() != expected_issuer_trimmed {
        return Err(());
    }

    Ok(())
}
/// `expected_issuer` (case-sensitive, after whitespace trimming on both sides).
///
/// This is a strict wrapper around [`verify_sep10_jwt`] for call sites where
/// the anchor identity is known ahead of time and must be explicitly validated
/// rather than merely present. All other checks from [`verify_sep10_jwt`]
/// apply unchanged.
///
/// # Arguments
///
/// * `env`             - Soroban execution environment.
/// * `token`           - Compact JWS token string.
/// * `anchor_public_key` - 32-byte Ed25519 public key.
/// * `expected_sub`    - Optional expected `sub` claim value.
/// * `expected_issuer` - The exact issuer string that the `iss` claim must match.
///
/// # Returns
///
/// `Ok(())` when all checks pass, `Err(())` otherwise.
pub fn verify_sep10_jwt_with_issuer(
    env: &Env,
    token: &String,
    anchor_public_key: &Bytes,
    expected_sub: Option<&String>,
    expected_issuer: &str,
) -> Result<(), ()> {
    if anchor_public_key.len() != 32 {
        return Err(());
    }

    // Reject an empty expected issuer immediately — a caller passing "" would
    // trivially accept any token and indicates a programming error.
    let expected_issuer_trimmed = expected_issuer.trim();
    if expected_issuer_trimmed.is_empty() {
        return Err(());
    }

    // Run base verification first.
    verify_sep10_jwt(env, token, anchor_public_key, expected_sub)?;

    // Decode payload again to extract iss for comparison.
    let max_len: u32 = env
        .storage()
        .instance()
        .get::<_, u32>(&soroban_sdk::symbol_short!("JWTMAXLEN"))
        .unwrap_or(MAX_JWT_LEN);

    let n = token.len() as usize;
    let mut buf: Vec<u8> = alloc::vec![0u8; max_len as usize];
    token.copy_into_slice(&mut buf[..n]);

    // Re-locate the dots to find the payload segment.
    let mut dots = [0usize; 2];
    let mut dot_count = 0usize;
    for i in 0..n {
        if buf[i] == b'.' {
            if dot_count < 2 {
                dots[dot_count] = i;
                dot_count += 1;
            }
        }
    }
    if dot_count != 2 {
        return Err(());
    }

    let payload_b64 = &buf[dots[0] + 1..dots[1]];
    let payload_dec = base64url_decode(payload_b64).map_err(|_| ())?;

    let iss_bytes = match parse_json_iss(&payload_dec) {
        Some(b) if !b.is_empty() => b,
        _ => return Err(()),
    };

    // Trim and compare — reject if the trimmed iss does not byte-equal the
    // trimmed expected_issuer.
    let iss_str = core::str::from_utf8(&iss_bytes).map_err(|_| ())?;
    if iss_str.trim() != expected_issuer_trimmed {
        return Err(());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;
    use std::string::ToString;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env};

    fn ledger(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
    }

    fn make_contract_id(env: &Env) -> Address {
        env.register_contract(None, crate::contract::AnchorKitContract)
    }

    fn build_jwt(signing_key: &SigningKey, sub: &str, exp: u64) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = r#"{"alg":"EdDSA","typ":"JWT"}"#;
        let iat = exp.saturating_sub(MAX_JWT_LIFETIME);
        let payload = format!(
            r#"{{"sub":"{}","iat":{},"exp":{},"iss":"https://anchor.example.com"}}"#,
            sub, iat, exp
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{}.{}", signing_input, sig_b64)
    }

    fn build_jwt_without_iat(signing_key: &SigningKey, sub: &str, exp: u64) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = r#"{"alg":"EdDSA","typ":"JWT"}"#;
        let payload = format!(
            r#"{{"sub":"{}","exp":{},"iss":"https://anchor.example.com"}}"#,
            sub, exp
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{}.{}", signing_input, sig_b64)
    }

    fn mutate_jwt_signature(jwt: &str) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

        let parts = jwt.split('.').collect::<std::vec::Vec<_>>();
        assert_eq!(parts.len(), 3);

        let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        sig[0] ^= 1;
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);

        format!("{}.{}.{}", parts[0], parts[1], sig_b64)
    }

    fn build_jwt_full(
        signing_key: &SigningKey,
        sub: &str,
        exp: u64,
        nbf: Option<u64>,
        jti: Option<&str>,
    ) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = r#"{"alg":"EdDSA","typ":"JWT"}"#;
        let iat = exp.saturating_sub(MAX_JWT_LIFETIME);
        let mut payload = format!(
            r#"{{"sub":"{}","iat":{},"exp":{},"iss":"https://anchor.example.com""#,
            sub, iat, exp
        );
        if let Some(n) = nbf {
            payload.push_str(&format!(r#","nbf":{}"#, n));
        }
        if let Some(j) = jti {
            payload.push_str(&format!(r#","jti":"{}""#, j));
        }
        payload.push('}');
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{}.{}", signing_input, sig_b64)
    }

    fn build_jwt_with_alg(
        signing_key: &SigningKey,
        alg: &str,
        sub: &str,
        exp: u64,
    ) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = format!(r#"{{"alg":"{}","typ":"JWT"}}"#, alg);
        let iat = exp.saturating_sub(MAX_JWT_LIFETIME);
        let payload = format!(
            r#"{{"sub":"{}","iat":{},"exp":{},"iss":"https://anchor.example.com"}}"#,
            sub, iat, exp
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{}.{}", signing_input, sig_b64)
    }

    fn build_jwt_no_iss(signing_key: &SigningKey, sub: &str, exp: u64) -> std::string::String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let header = r#"{"alg":"EdDSA","typ":"JWT"}"#;
        let iat = exp.saturating_sub(MAX_JWT_LIFETIME);
        let payload = format!(r#"{{"sub":"{}","iat":{},"exp":{}}}"#, sub, iat, exp);
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{}.{}", signing_input, sig_b64)
    }

    #[test]
    fn base64url_roundtrip_simple() {
        let dec = base64url_decode(b"SGVsbG8").unwrap();
        assert_eq!(dec, b"Hello");
    }

    #[test]
    fn verify_accepts_valid_token() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub = attestor.to_string();
        let sub_str: std::string::String = sub.to_string();
        let jwt = build_jwt(&signing_key, sub_str.as_str(), 2_000);
        let token = String::from_str(&env, jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, Some(&sub)).is_ok());
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_ok());
        });
    }

    #[test]
    fn verify_rejects_missing_iat_even_with_future_exp() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub = attestor.to_string();
        let sub_str: std::string::String = sub.to_string();
        let jwt = build_jwt_without_iat(&signing_key, sub_str.as_str(), 99_999_999_999);
        let token = String::from_str(&env, jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, Some(&sub)).is_err());
        });
    }

    #[test]
    fn verify_rejects_expired_token() {
        let env = Env::default();
        ledger(&env, 5_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub = attestor.to_string();
        let sub_str: std::string::String = sub.to_string();
        let jwt = build_jwt(&signing_key, sub_str.as_str(), 1_000);
        let token = String::from_str(&env, jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, Some(&sub)).is_err());
        });
    }

    #[test]
    #[should_panic]
    fn verify_rejects_invalid_signature() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let other_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, other_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub = attestor.to_string();
        let sub_str: std::string::String = sub.to_string();
        let jwt = build_jwt(&signing_key, sub_str.as_str(), 2_000);
        let token = String::from_str(&env, jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, Some(&sub)).is_err());
        });
    }

    #[test]
    #[should_panic]
    fn verify_rejects_mutated_signature() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub = attestor.to_string();
        let sub_str: std::string::String = sub.to_string();
        let jwt = build_jwt(&signing_key, sub_str.as_str(), 2_000);
        let tampered_jwt = mutate_jwt_signature(jwt.as_str());
        let token = String::from_str(&env, tampered_jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, Some(&sub)).is_err());
        });
    }

    #[test]
    fn verify_rejects_token_with_future_nbf() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_full(&signing_key, sub_str.as_str(), 5_000, Some(2_000), None);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_accepts_token_with_past_nbf() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_full(&signing_key, sub_str.as_str(), 5_000, Some(500), None);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_ok());
        });
    }

    #[test]
    fn verify_rejects_replayed_jti() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_full(&signing_key, sub_str.as_str(), 5_000, None, Some("unique-jti-abc"));
        let token = String::from_str(&env, jwt.as_str());

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_ok());
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_rejects_token_exceeding_default_max_len() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let long_sub = "G".repeat(2000);
        let jwt = build_jwt(&signing_key, &long_sub, 5_000);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_accepts_token_within_custom_max_len() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt2 = build_jwt(&signing_key, sub_str.as_str(), 5_000);
        let token2 = String::from_str(&env, jwt2.as_str());
        env.as_contract(&contract_id, || {
            env.storage()
                .instance()
                .set(&soroban_sdk::symbol_short!("JWTMAXLEN"), &8192u32);
            assert!(verify_sep10_jwt(&env, &token2, &pk, None).is_ok());
        });
    }

    #[test]
    fn verify_rejects_missing_iss_claim() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_no_iss(&signing_key, sub_str.as_str(), 5_000);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_rejects_rs256_algorithm() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        // RS256 header — should be rejected even if EdDSA is absent
        let jwt = build_jwt_with_alg(&signing_key, "RS256", sub_str.as_str(), 5_000);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_rejects_hs256_algorithm() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_with_alg(&signing_key, "HS256", sub_str.as_str(), 5_000);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    #[test]
    fn verify_rejects_none_algorithm() {
        let env = Env::default();
        ledger(&env, 1_000);
        let contract_id = make_contract_id(&env);
        let signing_key = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, signing_key.verifying_key().as_bytes());

        let attestor = Address::generate(&env);
        let sub_str: std::string::String = attestor.to_string().to_string();
        let jwt = build_jwt_with_alg(&signing_key, "none", sub_str.as_str(), 5_000);
        let token = String::from_str(&env, jwt.as_str());
        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }
}
