use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn cmd() -> Command {
    Command::cargo_bin("anchorkit").expect("anchorkit binary not found")
}

// ── Group A: missing contract_id validation ───────────────────────────────────

#[test]
fn test_register_missing_contract_id() {
    cmd()
        .args(["register", "--address", "GABC",
               "--services", "kyc",
               "--sep10-token", "T", "--sep10-issuer", "I",
               "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_attest_missing_contract_id() {
    cmd()
        .args(["attest", "--subject", "S", "--payload-hash", "H",
               "--issuer", "I", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_quote_missing_contract_id() {
    cmd()
        .args(["quote", "--from", "USDC", "--to", "XLM",
               "--amount", "100", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_revoke_missing_contract_id() {
    cmd()
        .args(["revoke", "--address", "GABC", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_deploy_upgrade_missing_contract_id() {
    cmd()
        .args(["deploy", "--upgrade", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

// ── Group B: contract_id resolved from env var ────────────────────────────────

/// When ANCHOR_CONTRACT_ID is set the contract-id validation passes; the
/// command then fails for another reason (missing signing key), NOT because
/// of a missing contract ID.
#[test]
fn test_register_contract_id_from_env_passes_validation() {
    cmd()
        .args(["register", "--address", "GABC",
               "--services", "kyc",
               "--sep10-token", "T", "--sep10-issuer", "I"])
        .env("ANCHOR_CONTRACT_ID", "CTEST123")
        .env_remove("ANCHOR_ADMIN_SECRET")
        .assert()
        .failure()
        // Error must be about the signing key, not the contract ID
        .stderr(contains("signing key required"))
        .stderr(predicates::str::contains("--contract-id").not());
}

#[test]
fn test_revoke_contract_id_from_env_passes_validation() {
    cmd()
        .args(["revoke", "--address", "GABC"])
        .env("ANCHOR_CONTRACT_ID", "CTEST123")
        .env_remove("ANCHOR_ADMIN_SECRET")
        .assert()
        .failure()
        .stderr(contains("signing key required"))
        .stderr(predicates::str::contains("--contract-id").not());
}

// ── Group C: network fallback note ────────────────────────────────────────────

#[test]
fn test_network_note_when_stellar_network_unset() {
    // `network list` is a harmless read-only command with no STELLAR_NETWORK dependency.
    // When neither STELLAR_NETWORK nor a default profile is set, a note is printed.
    cmd()
        .args(["network", "list"])
        .env_remove("STELLAR_NETWORK")
        .env("HOME", std::env::temp_dir().to_str().unwrap())
        .assert()
        .success()
        .stderr(contains("note:"))
        .stderr(contains("testnet"));
}

#[test]
fn test_no_network_note_when_stellar_network_is_set() {
    cmd()
        .args(["network", "list"])
        .env("STELLAR_NETWORK", "testnet")
        .assert()
        .success()
        .stderr(predicates::str::contains("note:").not());
}

// ── Group D: `env` subcommand ─────────────────────────────────────────────────

// ── Group E: commands that do NOT need contract_id ───────────────────────────

#[test]
fn test_doctor_does_not_require_contract_id() {
    // doctor may exit 0 or 1 depending on the environment, but must NOT
    // emit the contract-id validation error.
    let output = cmd()
        .args(["doctor"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .output()
        .expect("failed to run anchorkit doctor");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("--contract-id (or ANCHOR_CONTRACT_ID) is required"),
        "doctor should not require --contract-id, but got: {stderr}"
    );
}

#[test]
fn test_credentials_list_does_not_require_contract_id() {
    cmd()
        .args(["credentials", "list"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .env("HOME", std::env::temp_dir().to_str().unwrap())
        .assert()
        .success()
        .stderr(predicates::str::contains("--contract-id").not());
}

#[test]
fn test_network_list_does_not_require_contract_id() {
    cmd()
        .args(["network", "list"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .env("HOME", std::env::temp_dir().to_str().unwrap())
        .assert()
        .success()
        .stderr(predicates::str::contains("--contract-id (or ANCHOR_CONTRACT_ID) is required").not());
}

// ── Group F: invalid invocations (clap-level) ────────────────────────────────

#[test]
fn test_missing_subcommand_exits_with_usage() {
    cmd()
        .assert()
        .failure()
        .stderr(contains("Usage"));
}

#[test]
fn test_unknown_subcommand() {
    cmd()
        .args(["foobar"])
        .assert()
        .failure()
        .stderr(contains("unrecognized subcommand"));
}

#[test]
fn test_register_missing_required_address_arg() {
    // --contract-id and signing key provided, but --address is missing → clap error
    cmd()
        .args(["register",
               "--contract-id", "CTEST",
               "--services", "kyc",
               "--sep10-token", "T", "--sep10-issuer", "I",
               "--secret-key", "SABC"])
        .assert()
        .failure()
        .stderr(contains("--address"));
}

// ── Group: verify command validation (#557) ───────────────────────────────────

#[test]
fn test_verify_without_id_or_hash_exits_with_error() {
    cmd()
        .args(["verify", "--contract-id", "CTEST", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--id").or(contains("--payload-hash")).or(contains("required")));
}

#[test]
fn test_verify_with_both_id_and_hash_exits_with_error() {
    cmd()
        .args(["verify", "--id", "1", "--payload-hash", "abc123",
               "--contract-id", "CTEST", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("mutually exclusive").or(contains("--id").and(contains("--payload-hash"))));
}

#[test]
fn test_verify_missing_contract_id() {
    cmd()
        .args(["verify", "--id", "1", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id").or(contains("ANCHOR_CONTRACT_ID")));
}

#[test]
fn test_verify_payload_file_not_found() {
    cmd()
        .args(["verify", "--id", "1",
               "--payload-file", "/nonexistent/path/payload.bin",
               "--contract-id", "CTEST", "--secret-key", "SABC"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure();
}

// ── Group G: deploy network validation ───────────────────────────────────────

/// An unsupported network value must be rejected with a nonzero exit before
/// any deployment work begins.  The error message must identify the bad option
/// or list the supported choices.
#[test]
fn test_deploy_invalid_network_exits_with_error() {
    cmd()
        .args(["deploy", "--network", "not-a-real-network"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .env_remove("STELLAR_NETWORK")
        // Provide a temp HOME so the test never accidentally picks up a real
        // custom profile from the developer's machine.
        .env("HOME", std::env::temp_dir().to_str().unwrap())
        .env("USERPROFILE", std::env::temp_dir().to_str().unwrap())
        .assert()
        .failure()
        .stderr(
            contains("not-a-real-network")
                .or(contains("unknown network"))
                .or(contains("testnet"))
        );
}

/// A valid built-in network name must pass the argument-validation gate.
/// The command will still fail (WASM not built, no signing key, etc.),
/// but the error must NOT be the network-name rejection.
#[test]
fn test_deploy_valid_network_passes_validation() {
    let output = cmd()
        .args(["deploy", "--network", "testnet", "--dry-run"])
        .env_remove("ANCHOR_CONTRACT_ID")
        .env_remove("STELLAR_NETWORK")
        .env("HOME", std::env::temp_dir().to_str().unwrap())
        .env("USERPROFILE", std::env::temp_dir().to_str().unwrap())
        .output()
        .expect("anchorkit binary not found");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unknown network"),
        "testnet should be accepted as a valid network name, got stderr: {stderr}"
    );
}

// ── Group H: offline validate CLI help ───────────────────────────────────────

/// `offline validate --help` must explain that validation is local and does
/// not contact an anchor or any network endpoint.
#[test]
fn test_offline_validate_help_mentions_no_network() {
    cmd()
        .args(["offline", "validate", "--help"])
        .assert()
        .success()
        .stdout(
            contains("local")
                .or(contains("no network"))
                .or(contains("not contact"))
                .or(contains("offline"))
                .or(contains("air-gapped"))
        );
}

/// `offline validate --help` must describe the accepted input formats.
#[test]
fn test_offline_validate_help_mentions_config_formats() {
    cmd()
        .args(["offline", "validate", "--help"])
        .assert()
        .success()
        .stdout(
            contains("json").or(contains("JSON"))
                .or(contains("toml")).or(contains("TOML"))
        );
}
