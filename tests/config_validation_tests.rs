#![cfg(feature = "std")]

use std::{fs, path::PathBuf};

use anchorkit::{load_runtime_config_file, parse_runtime_config_str, ConfigFormat};
use jsonschema::JSONSchema;
use serde_json::Value;

const SCHEMA: &str = "config_schema.json";
const CONFIG_DIR: &str = "configs";

fn config_paths() -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(CONFIG_DIR)
        .expect("configs directory should exist")
        .map(|entry| entry.expect("config entry should be readable").path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("json" | "toml")
                )
        })
        .collect();
    paths.sort();
    paths
}

fn config_to_json_value(path: &PathBuf) -> Value {
    let input = fs::read_to_string(path).expect("config should be readable");
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => serde_json::from_str(&input).expect("JSON config should parse"),
        Some("toml") => {
            let value: toml::Value = toml::from_str(&input).expect("TOML config should parse");
            serde_json::to_value(value).expect("TOML config should convert to JSON value")
        }
        _ => panic!("unsupported config extension: {}", path.display()),
    }
}

fn compiled_schema() -> JSONSchema {
    let schema_text = fs::read_to_string(SCHEMA).expect("schema should be readable");
    let schema_json: Value = serde_json::from_str(&schema_text).expect("schema should be JSON");
    JSONSchema::compile(&schema_json).expect("schema should compile as JSON Schema")
}

#[test]
fn config_schema_is_valid_json_schema() {
    let _ = compiled_schema();
}

#[test]
fn all_example_configs_validate_against_schema() {
    let schema = compiled_schema();
    let paths = config_paths();
    assert!(!paths.is_empty(), "expected at least one example config");

    for path in paths {
        let value = config_to_json_value(&path);
        let validation = schema.validate(&value);
        let errors = match validation {
            Ok(()) => Vec::new(),
            Err(errors) => errors.map(|err| err.to_string()).collect(),
        };
        assert!(
            errors.is_empty(),
            "{} failed schema validation:\n{}",
            path.display(),
            errors.join("\n")
        );
    }
}

#[test]
fn all_example_configs_load_with_runtime_parser() {
    let paths = config_paths();
    assert!(!paths.is_empty(), "expected at least one example config");

    for path in paths {
        load_runtime_config_file(&path)
            .unwrap_or_else(|err| panic!("{} failed runtime parsing: {err}", path.display()));
    }
}

#[test]
fn runtime_parser_rejects_unknown_top_level_section() {
    let bad = r#"{
  "contract": { "name": "bad-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  },
  "unsupported": { "value": true }
}"#;

    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "unsupported top-level section should be rejected");
}

#[test]
fn runtime_parser_rejects_malformed_nested_shape() {
    let bad = r#"{
  "contract": { "name": "bad-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true,
      "mystery_flag": true
    }]
  }
}"#;

    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "unknown nested attestor field should be rejected");
}

#[test]
fn runtime_parser_rejects_unknown_attestor_references() {
    let bad = r#"{
  "contract": { "name": "bad-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  },
  "operations": {
    "templates": [{
      "id": "missing-attestor",
      "name": "Missing Attestor",
      "attestor": "not-registered",
      "operation_type": "kyc",
      "required_fields": ["user_id"],
      "replay_protection": "enabled"
    }]
  }
}"#;

    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "unknown operation attestor should be rejected");
}

// ── Whitespace-name trimming ──────────────────────────────────────────────────

#[test]
fn whitespace_only_contract_name_is_rejected() {
    let bad = r#"{
  "contract": { "name": "   ", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  }
}"#;
    // The schema enforces pattern ^[a-z0-9-]+$ so whitespace is also caught
    // at the schema layer; the semantic layer provides the trim-aware message.
    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "whitespace-only contract name should be rejected");
}

#[test]
fn ordinary_contract_name_is_accepted() {
    let good = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  }
}"#;
    let result = parse_runtime_config_str(good, ConfigFormat::Json);
    assert!(result.is_ok(), "ordinary name should be accepted: {:?}", result);
}

// ── Duplicate attestor names ──────────────────────────────────────────────────

#[test]
fn duplicate_attestor_name_is_rejected() {
    let bad = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [
      {
        "name": "kyc-issuer",
        "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
        "role": "kyc-issuer",
        "enabled": true
      },
      {
        "name": "kyc-issuer",
        "address": "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGBM3NMKL3YEI6CGMFA8QQ",
        "role": "transfer-verifier",
        "enabled": true
      }
    ]
  }
}"#;
    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "duplicate attestor name should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.contains("kyc-issuer"),
        "error should identify the conflicting name, got: {err}"
    );
}

#[test]
fn unique_attestor_names_are_accepted() {
    let good = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [
      {
        "name": "kyc-issuer",
        "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
        "role": "kyc-issuer",
        "enabled": true
      },
      {
        "name": "transfer-verifier",
        "address": "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGBM3NMKL3YEI6CGMFA8QQ",
        "role": "transfer-verifier",
        "enabled": true
      }
    ]
  }
}"#;
    let result = parse_runtime_config_str(good, ConfigFormat::Json);
    assert!(result.is_ok(), "unique attestor names should be accepted: {:?}", result);
}

// ── Schema validation (validate_against_schema) ───────────────────────────────

#[test]
fn schema_rejects_wrong_type_for_contract_field() {
    // "version" must be a string, not an integer
    let bad = r#"{
  "contract": { "name": "my-anchor", "version": 1, "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  }
}"#;
    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "wrong type for version should be rejected by schema");
}

#[test]
fn schema_rejects_missing_required_property() {
    // "network" is required under contract
    let bad = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  }
}"#;
    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "missing required 'network' field should be rejected by schema");
}

#[test]
fn schema_rejects_unknown_top_level_property() {
    let bad = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  },
  "unknown_field": "should_fail"
}"#;
    let result = parse_runtime_config_str(bad, ConfigFormat::Json);
    assert!(result.is_err(), "unknown top-level field should be rejected by schema");
}

#[test]
fn valid_config_still_parses_through_schema_path() {
    let good = r#"{
  "contract": { "name": "my-anchor", "version": "1.0.0", "network": "stellar-testnet" },
  "attestors": {
    "registry": [{
      "name": "kyc-issuer",
      "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
      "role": "kyc-issuer",
      "enabled": true
    }]
  }
}"#;
    let result = parse_runtime_config_str(good, ConfigFormat::Json);
    assert!(result.is_ok(), "valid config should pass schema validation: {:?}", result);
}
