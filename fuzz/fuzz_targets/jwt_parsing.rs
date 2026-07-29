#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt to parse arbitrary bytes as base64url-encoded JWT component
    if let Ok(decoded) = anchorkit::sep10_jwt::base64url_decode(data) {
        // If decoded successfully, ensure it's valid UTF-8 or binary
        let _result = std::str::from_utf8(&decoded);
    }

    // Also test with UTF-8 strings (common JWT format)
    if let Ok(s) = std::str::from_utf8(data) {
        // Test base64url decoding on string input
        let bytes = s.as_bytes();
        let _result = anchorkit::sep10_jwt::base64url_decode(bytes);
    }
});
