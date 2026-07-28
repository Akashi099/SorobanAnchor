#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt to parse arbitrary bytes as JSON config
    if let Ok(s) = std::str::from_utf8(data) {
        // Try JSON format
        if let Ok(_value) = serde_json::from_str::<serde_json::Value>(s) {
            let _config = anchorkit::parse_runtime_config_str(s, anchorkit::ConfigFormat::Json);
        }

        // Try TOML format
        if let Ok(_value) = toml::from_str::<toml::Value>(s) {
            let _config = anchorkit::parse_runtime_config_str(s, anchorkit::ConfigFormat::Toml);
        }
    }
});
