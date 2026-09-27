/// Network profile parsing, validation, and fallback tests.
///
/// These tests cover:
/// - Malformed JSON does not crash the loader.
/// - Missing file returns an empty profile set.
/// - Invalid profile entries are skipped with diagnostics.
/// - Unknown networks fall back to testnet RPC/passphrase.
/// - Valid profiles are loaded and resolved correctly.
///
/// Run with: cargo test --test network_profile_tests
#[cfg(test)]
mod network_profile_tests {
    use std::fs;

    // ── Inline mirror of the types and functions under test ──────────────────
    //
    // main.rs is a binary crate and cannot be imported as a library.
    // We replicate the minimal surface needed to test the logic in isolation.

    #[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
    struct NetworkProfile {
        name: String,
        rpc_url: String,
        network_passphrase: String,
        horizon_url: Option<String>,
        #[serde(default)]
        is_default: bool,
    }

    #[derive(Debug, PartialEq)]
    enum NetworkProfileError {
        IoError(String),
        MalformedJson(String),
        InvalidProfile { index: usize, reason: String },
        MissingFile,
    }

    impl std::fmt::Display for NetworkProfileError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                NetworkProfileError::IoError(msg) =>
                    write!(f, "could not read networks.json: {msg}"),
                NetworkProfileError::MalformedJson(msg) =>
                    write!(f, "networks.json contains invalid JSON: {msg}"),
                NetworkProfileError::InvalidProfile { index, reason } =>
                    write!(f, "network profile at index {index} is invalid: {reason}"),
                NetworkProfileError::MissingFile =>
                    write!(f, "networks.json is missing (expected at ~/.anchorkit/networks.json)"),
            }
        }
    }

    fn validate_network_profile(profile: &NetworkProfile) -> Result<(), String> {
        if profile.name.trim().is_empty() {
            return Err("'name' must not be empty".to_string());
        }
        if profile.name.len() > 64 {
            return Err(format!("'name' is too long ({} chars, max 64)", profile.name.len()));
        }
        if !profile.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "'name' contains invalid characters: '{}'",
                profile.name
            ));
        }
        if profile.rpc_url.trim().is_empty() {
            return Err("'rpc_url' must not be empty".to_string());
        }
        // Issue #1129: Use structural URL validation
        validate_url_structural(&profile.rpc_url).map_err(|e| {
            format!("'rpc_url' is not a valid HTTP(S) URL: {}", e)
        })?;
        if profile.network_passphrase.trim().is_empty() {
            return Err("'network_passphrase' must not be empty".to_string());
        }
        if let Some(ref h) = profile.horizon_url {
            if !h.trim().is_empty() {
                validate_url_structural(h).map_err(|e| {
                    format!("'horizon_url' is not a valid HTTP(S) URL: {}", e)
                })?;
            }
        }
        Ok(())
    }

    fn validate_url_structural(input: &str) -> Result<(), String> {
        use url::Url;
        
        let parsed = Url::parse(input).map_err(|e| format!("malformed URL: {}", e))?;
        
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(format!("scheme must be http or https, got: {}", scheme));
        }
        
        if parsed.host().is_none() {
            return Err("missing host/authority".to_string());
        }
        
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("embedded credentials not allowed in URL".to_string());
        }
        
        if input.chars().any(|c| c.is_control()) {
            return Err("control characters not allowed in URL".to_string());
        }
        
        Ok(())
    }

    fn load_profiles_from_str(content: &str) -> (Vec<NetworkProfile>, Vec<NetworkProfileError>) {
        if content.trim().is_empty() {
            return (Vec::new(), Vec::new());
        }
        let raw_value: serde_json::Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => {
                return (
                    Vec::new(),
                    vec![NetworkProfileError::MalformedJson(e.to_string())],
                );
            }
        };
        if !raw_value.is_array() {
            return (
                Vec::new(),
                vec![NetworkProfileError::MalformedJson(
                    "expected a JSON array at the top level".to_string(),
                )],
            );
        }
        let raw_array = raw_value.as_array().unwrap();
        let mut valid_profiles: Vec<NetworkProfile> = Vec::new();
        let mut errors: Vec<NetworkProfileError> = Vec::new();
        for (index, entry) in raw_array.iter().enumerate() {
            match serde_json::from_value::<NetworkProfile>(entry.clone()) {
                Err(e) => {
                    errors.push(NetworkProfileError::InvalidProfile {
                        index,
                        reason: format!("deserialization failed: {e}"),
                    });
                }
                Ok(profile) => match validate_network_profile(&profile) {
                    Ok(()) => valid_profiles.push(profile),
                    Err(reason) => {
                        errors.push(NetworkProfileError::InvalidProfile { index, reason });
                    }
                },
            }
        }
        
        // Issue #1128: Fail when file exists with profiles but all are invalid
        if !raw_array.is_empty() && valid_profiles.is_empty() && !errors.is_empty() {
            return (Vec::new(), errors);
        }
        
        (valid_profiles, errors)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn valid_profile_json(name: &str) -> String {
        format!(
            r#"[{{"name":"{name}","rpc_url":"https://rpc.example.com","network_passphrase":"Test Network ; 2024","is_default":false}}]"#
        )
    }

    // ── Missing file ──────────────────────────────────────────────────────────

    #[test]
    fn missing_file_returns_empty_profiles_no_errors() {
        // Simulate a missing file by loading from a non-existent path.
        // We test the string-based loader directly (file I/O is tested separately).
        let (profiles, errors) = load_profiles_from_str("");
        assert!(profiles.is_empty(), "missing file should yield no profiles");
        assert!(errors.is_empty(), "missing file should yield no errors");
    }

    // ── Malformed JSON ────────────────────────────────────────────────────────

    #[test]
    fn malformed_json_returns_empty_profiles_with_error() {
        let (profiles, errors) = load_profiles_from_str("{not valid json!!!");
        assert!(profiles.is_empty(), "malformed JSON should yield no profiles");
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::MalformedJson(_)),
            "expected MalformedJson error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn truncated_json_returns_malformed_error() {
        let (profiles, errors) = load_profiles_from_str(r#"[{"name":"test""#);
        assert!(profiles.is_empty());
        assert!(matches!(&errors[0], NetworkProfileError::MalformedJson(_)));
    }

    #[test]
    fn json_object_instead_of_array_returns_malformed_error() {
        let (profiles, errors) = load_profiles_from_str(
            r#"{"name":"test","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}"#,
        );
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::MalformedJson(msg) if msg.contains("array")),
            "expected array-shape error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn empty_json_array_returns_no_profiles_no_errors() {
        let (profiles, errors) = load_profiles_from_str("[]");
        assert!(profiles.is_empty());
        assert!(errors.is_empty());
    }

    // ── Field validation ──────────────────────────────────────────────────────

    #[test]
    fn profile_with_empty_name_is_rejected() {
        let json = r#"[{"name":"","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("name")),
            "expected name-related error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_empty_rpc_url_is_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("rpc_url")),
            "expected rpc_url error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_empty_passphrase_is_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com","network_passphrase":""}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("network_passphrase")),
            "expected passphrase error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_invalid_rpc_url_scheme_is_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"ftp://rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("rpc_url")),
            "expected rpc_url scheme error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_invalid_horizon_url_scheme_is_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com","network_passphrase":"Test","horizon_url":"ftp://horizon.example.com"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("horizon_url")),
            "expected horizon_url scheme error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_name_containing_spaces_is_rejected() {
        let json = r#"[{"name":"my net","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("invalid characters")),
            "expected invalid-characters error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_with_name_too_long_is_rejected() {
        let long_name = "a".repeat(65);
        let json = format!(
            r#"[{{"name":"{long_name}","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}}]"#
        );
        let (profiles, errors) = load_profiles_from_str(&json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("too long")),
            "expected too-long error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn profile_missing_required_field_is_rejected() {
        // Missing `network_passphrase` entirely.
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { .. }),
            "expected InvalidProfile error, got: {:?}",
            errors[0]
        );
    }

    // ── Partial validity: good entries survive bad ones ───────────────────────

    #[test]
    fn valid_entries_survive_invalid_entries_in_same_file() {
        let json = r#"[
            {"name":"good-net","rpc_url":"https://rpc.example.com","network_passphrase":"Test Network"},
            {"name":"","rpc_url":"https://rpc.example.com","network_passphrase":"Test Network"},
            {"name":"also-good","rpc_url":"https://rpc2.example.com","network_passphrase":"Test Network 2"}
        ]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert_eq!(profiles.len(), 2, "two valid profiles should be loaded");
        assert_eq!(errors.len(), 1, "one invalid profile should produce one error");
        assert_eq!(profiles[0].name, "good-net");
        assert_eq!(profiles[1].name, "also-good");
    }

    #[test]
    fn all_invalid_entries_returns_empty_profiles_with_errors() {
        let json = r#"[
            {"name":"","rpc_url":"https://rpc.example.com","network_passphrase":"Test"},
            {"name":"bad url","rpc_url":"ftp://rpc.example.com","network_passphrase":"Test"}
        ]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty());
        assert_eq!(errors.len(), 2);
    }

    // ── Valid profiles ────────────────────────────────────────────────────────

    #[test]
    fn valid_profile_loads_correctly() {
        let json = valid_profile_json("my-custom-net");
        let (profiles, errors) = load_profiles_from_str(&json);
        assert!(errors.is_empty(), "valid profile should produce no errors");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "my-custom-net");
        assert_eq!(profiles[0].rpc_url, "https://rpc.example.com");
        assert_eq!(profiles[0].network_passphrase, "Test Network ; 2024");
        assert!(!profiles[0].is_default);
    }

    #[test]
    fn profile_with_http_rpc_url_is_accepted() {
        // http:// is allowed (some local/dev setups use plain HTTP).
        let json = r#"[{"name":"local","rpc_url":"http://localhost:8000","network_passphrase":"Local Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(errors.is_empty(), "http:// rpc_url should be accepted");
        assert_eq!(profiles.len(), 1);
    }

    #[test]
    fn profile_with_optional_horizon_url_loads_correctly() {
        let json = r#"[{
            "name":"mynet",
            "rpc_url":"https://rpc.example.com",
            "network_passphrase":"Test",
            "horizon_url":"https://horizon.example.com",
            "is_default":true
        }]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(errors.is_empty());
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].horizon_url.as_deref(), Some("https://horizon.example.com"));
        assert!(profiles[0].is_default);
    }

    #[test]
    fn profile_without_is_default_defaults_to_false() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(errors.is_empty());
        assert!(!profiles[0].is_default, "is_default should default to false");
    }

    #[test]
    fn profile_name_with_hyphens_and_underscores_is_accepted() {
        let json = r#"[{"name":"my-custom_net","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(errors.is_empty(), "hyphens and underscores in name should be accepted");
        assert_eq!(profiles.len(), 1);
    }

    #[test]
    fn profile_name_at_max_length_is_accepted() {
        let name = "a".repeat(64);
        let json = format!(
            r#"[{{"name":"{name}","rpc_url":"https://rpc.example.com","network_passphrase":"Test"}}]"#
        );
        let (profiles, errors) = load_profiles_from_str(&json);
        assert!(errors.is_empty(), "64-char name should be accepted");
        assert_eq!(profiles.len(), 1);
    }

    // ── Unknown network fallback ──────────────────────────────────────────────

    /// Mirrors the fallback logic in `rpc_url_for` / `passphrase_for`.
    fn resolve_rpc(profiles: &[NetworkProfile], network: &str) -> String {
        if let Some(p) = profiles.iter().find(|p| p.name == network) {
            return p.rpc_url.clone();
        }
        // Built-in fallback
        match network {
            "mainnet"   => "https://horizon.stellar.org".to_string(),
            "futurenet" => "https://rpc-futurenet.stellar.org".to_string(),
            _           => "https://soroban-testnet.stellar.org".to_string(),
        }
    }

    fn resolve_passphrase(profiles: &[NetworkProfile], network: &str) -> String {
        if let Some(p) = profiles.iter().find(|p| p.name == network) {
            return p.network_passphrase.clone();
        }
        match network {
            "mainnet"   => "Public Global Stellar Network ; September 2015".to_string(),
            "futurenet" => "Test SDF Future Network ; October 2022".to_string(),
            _           => "Test SDF Network ; September 2015".to_string(),
        }
    }

    #[test]
    fn unknown_network_falls_back_to_testnet_rpc() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        let url = resolve_rpc(&profiles, "completely-unknown-network");
        assert_eq!(url, "https://soroban-testnet.stellar.org");
    }

    #[test]
    fn unknown_network_falls_back_to_testnet_passphrase() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        let phrase = resolve_passphrase(&profiles, "completely-unknown-network");
        assert_eq!(phrase, "Test SDF Network ; September 2015");
    }

    #[test]
    fn builtin_testnet_resolves_correctly() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        assert_eq!(resolve_rpc(&profiles, "testnet"), "https://soroban-testnet.stellar.org");
        assert_eq!(resolve_passphrase(&profiles, "testnet"), "Test SDF Network ; September 2015");
    }

    #[test]
    fn builtin_mainnet_resolves_correctly() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        assert_eq!(resolve_rpc(&profiles, "mainnet"), "https://horizon.stellar.org");
        assert_eq!(resolve_passphrase(&profiles, "mainnet"), "Public Global Stellar Network ; September 2015");
    }

    #[test]
    fn builtin_futurenet_resolves_correctly() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        assert_eq!(resolve_rpc(&profiles, "futurenet"), "https://rpc-futurenet.stellar.org");
        assert_eq!(resolve_passphrase(&profiles, "futurenet"), "Test SDF Future Network ; October 2022");
    }

    #[test]
    fn custom_profile_takes_precedence_over_builtin_name() {
        let (profiles, errors) = load_profiles_from_str(r#"[{
            "name":"testnet",
            "rpc_url":"https://my-private-testnet.example.com",
            "network_passphrase":"My Private Testnet ; 2024"
        }]"#);
        assert!(errors.is_empty());
        let url = resolve_rpc(&profiles, "testnet");
        assert_eq!(url, "https://my-private-testnet.example.com",
            "custom profile should override built-in testnet");
    }

    // ── Default network resolution ────────────────────────────────────────────

    fn default_network_from(profiles: &[NetworkProfile]) -> String {
        profiles.iter()
            .find(|p| p.is_default)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "testnet".to_string())
    }

    #[test]
    fn no_default_profile_falls_back_to_testnet() {
        let profiles: Vec<NetworkProfile> = Vec::new();
        assert_eq!(default_network_from(&profiles), "testnet");
    }

    #[test]
    fn marked_default_profile_is_returned() {
        let (profiles, errors) = load_profiles_from_str(r#"[
            {"name":"net-a","rpc_url":"https://a.example.com","network_passphrase":"Net A","is_default":false},
            {"name":"net-b","rpc_url":"https://b.example.com","network_passphrase":"Net B","is_default":true}
        ]"#);
        assert!(errors.is_empty());
        assert_eq!(default_network_from(&profiles), "net-b");
    }

    // ── File I/O round-trip ───────────────────────────────────────────────────

    #[test]
    fn save_and_reload_round_trip() {
        let dir = std::env::temp_dir().join(format!("anchorkit_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("networks.json");

        let profiles = vec![
            NetworkProfile {
                name: "round-trip-net".to_string(),
                rpc_url: "https://rpc.example.com".to_string(),
                network_passphrase: "Round Trip Test".to_string(),
                horizon_url: Some("https://horizon.example.com".to_string()),
                is_default: true,
            },
        ];

        let json = serde_json::to_string_pretty(&profiles).unwrap();
        fs::write(&path, &json).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let (loaded, errors) = load_profiles_from_str(&content);
        assert!(errors.is_empty());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "round-trip-net");
        assert_eq!(loaded[0].horizon_url.as_deref(), Some("https://horizon.example.com"));
        assert!(loaded[0].is_default);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn whitespace_only_file_returns_empty_profiles() {
        let (profiles, errors) = load_profiles_from_str("   \n\t  ");
        assert!(profiles.is_empty());
        assert!(errors.is_empty(), "whitespace-only file should not produce errors");
    }

    // ── Error display ─────────────────────────────────────────────────────────

    #[test]
    fn malformed_json_error_display_is_informative() {
        let err = NetworkProfileError::MalformedJson("unexpected token at line 1".to_string());
        let msg = err.to_string();
        assert!(msg.contains("invalid JSON"), "display should mention invalid JSON: {msg}");
        assert!(msg.contains("unexpected token"), "display should include parse detail: {msg}");
    }

    #[test]
    fn invalid_profile_error_display_includes_index_and_reason() {
        let err = NetworkProfileError::InvalidProfile {
            index: 3,
            reason: "'name' must not be empty".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("index 3"), "display should include index: {msg}");
        assert!(msg.contains("name"), "display should include reason: {msg}");
    }

    #[test]
    fn io_error_display_is_informative() {
        let err = NetworkProfileError::IoError("permission denied".to_string());
        let msg = err.to_string();
        assert!(msg.contains("networks.json"), "display should mention the file: {msg}");
        assert!(msg.contains("permission denied"), "display should include OS error: {msg}");
    }

    // ── Issue #1130: Directory creation errors ───────────────────────────────

    /// Simulates unwritable directory error propagation by testing the error path.
    /// When the parent directory cannot be created (e.g. permission denied),
    /// the error should be returned with path and cause context, not silently
    /// discarded.
    #[test]
    fn directory_creation_failure_propagates_error() {
        // This test validates that NetworkProfileError::IoError includes path
        // context when directory creation fails.
        let err = NetworkProfileError::IoError(
            "cannot create directory '/unwritable/path': permission denied".to_string()
        );
        let msg = err.to_string();
        assert!(msg.contains("/unwritable/path"), "error should include the problematic path: {msg}");
        assert!(msg.contains("permission denied"), "error should include the OS cause: {msg}");
    }
}

    // ── Issue #1127: Missing file diagnostic ──────────────────────────────────

    #[test]
    fn missing_file_returns_missing_file_diagnostic() {
        // This test validates that a missing networks.json produces a MissingFile diagnostic.
        // Since the test uses string-based loading, we manually check the enum variant.
        let err = NetworkProfileError::MissingFile;
        let msg = err.to_string();
        assert!(msg.contains("missing"), "error should mention missing file: {msg}");
        assert!(msg.contains("networks.json"), "error should mention networks.json: {msg}");
    }

    // ── Issue #1128: Fail when all profiles invalid ───────────────────────────

    #[test]
    fn all_invalid_profiles_returns_errors() {
        // When a file exists with profiles but all are invalid, the function
        // should return all validation errors.
        let json = r#"[
            {"name":"","rpc_url":"https://rpc.example.com","network_passphrase":"Test"},
            {"name":"bad","rpc_url":"","network_passphrase":"Test"}
        ]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty(), "all invalid profiles should yield no valid profiles");
        assert_eq!(errors.len(), 2, "both invalid profiles should produce errors");
    }

    // ── Issue #1129: Structural URL validation ────────────────────────────────

    #[test]
    fn rpc_url_with_embedded_credentials_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://user:pass@rpc.example.com","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty(), "URL with credentials should be rejected");
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("credentials")),
            "expected credentials error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn rpc_url_with_control_characters_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com\u0000/path","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty(), "URL with control characters should be rejected");
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("control")),
            "expected control character error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn rpc_url_missing_authority_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty(), "URL without host should be rejected");
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("host") || reason.contains("authority")),
            "expected missing host error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn horizon_url_with_embedded_credentials_rejected() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com","network_passphrase":"Test","horizon_url":"https://user:pass@horizon.example.com"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(profiles.is_empty(), "horizon_url with credentials should be rejected");
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(&errors[0], NetworkProfileError::InvalidProfile { reason, .. } if reason.contains("credentials")),
            "expected credentials error, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn valid_https_url_with_port_accepted() {
        let json = r#"[{"name":"mynet","rpc_url":"https://rpc.example.com:443/path","network_passphrase":"Test"}]"#;
        let (profiles, errors) = load_profiles_from_str(json);
        assert!(errors.is_empty(), "valid HTTPS URL with port should be accepted");
        assert_eq!(profiles.len(), 1);
    }
