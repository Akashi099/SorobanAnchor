#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    // Attempt to parse arbitrary bytes as JSON
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(value) = serde_json::from_str::<Value>(s) {
            // If it parses, verify it can be serialized back
            let _serialized = serde_json::to_string(&value);
        }
    }
});
