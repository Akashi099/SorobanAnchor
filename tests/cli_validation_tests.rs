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
        .args([
            "register",
            "--address",
            "GABC",
            "--services",
            "kyc",
            "--sep10-token",
            "T",
            "--sep10-issuer",
            "I",
            "--secret-key",
            "SABC",
        ])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_attest_missing_contract_id() {
    cmd()
        .args([
            "attest",
            "--subject",
            "S",
            "--payload-hash",
            "H",
            "--issuer",
            "I",
            "--secret-key",
            "SABC",
        ])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(contains("--contract-id"))
        .stderr(contains("ANCHOR_CONTRACT_ID"));
}

#[test]
fn test_quote_missing_contract_id() {
    cmd()
        .args([
            "quote",
            "--from",
            "USDC",
            "--to",
            "XLM",
            "--amount",
            "100",
            "--secret-key",
            "SABC",
        ])
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
        .stderr(
            predicates::str::contains("--contract-id (or ANCHOR_CONTRACT_ID) is required").not(),
        );
}

// ── Group F: invalid invocations (clap-level) ────────────────────────────────

#[test]
fn test_missing_subcommand_exits_with_usage() {
    cmd().assert().failure().stderr(contains("Usage"));
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
        .args([
            "register",
            "--contract-id",
            "CTEST",
            "--services",
            "kyc",
            "--sep10-token",
            "T",
            "--sep10-issuer",
            "I",
            "--secret-key",
            "SABC",
        ])
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
        .stderr(
            contains("--id")
                .or(contains("--payload-hash"))
                .or(contains("required")),
        );
}

#[test]
fn test_verify_with_both_id_and_hash_exits_with_error() {
    cmd()
        .args([
            "verify",
            "--id",
            "1",
            "--payload-hash",
            "abc123",
            "--contract-id",
            "CTEST",
            "--secret-key",
            "SABC",
        ])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure()
        .stderr(
            contains("mutually exclusive").or(contains("--id").and(contains("--payload-hash"))),
        );
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
        .args([
            "verify",
            "--id",
            "1",
            "--payload-file",
            "/nonexistent/path/payload.bin",
            "--contract-id",
            "CTEST",
            "--secret-key",
            "SABC",
        ])
        .env_remove("ANCHOR_CONTRACT_ID")
        .assert()
        .failure();
}

// ── Group G: offline validate — nonexistent config path ──────────────────────

/// Invoking `offline validate --config <nonexistent>` must:
///   - Exit with a nonzero status (never panic).
///   - Print or emit something that mentions the supplied path so the
///     operator knows exactly which file could not be read.
///   - Pass on any machine regardless of filesystem layout (the path is
///     constructed in the OS temp directory so it cannot accidentally exist).
#[test]
fn test_offline_validate_nonexistent_config_fails_with_path_in_output() {
    // Build a path that is guaranteed never to exist: a uniquely-named file
    // under the OS temp directory.  We deliberately do NOT create it.
    let absent_path = std::env::temp_dir().join(format!(
        "anchorkit_test_absent_config_{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(12345)
    ));

    let path_str = absent_path
        .to_str()
        .expect("temp path should be valid UTF-8");

    // The `offline validate --config <path>` subcommand tries to read the
    // file; it must fail cleanly rather than panic.
    let output = cmd()
        .args(["offline", "validate", "--config", path_str])
        .output()
        .expect("failed to spawn anchorkit process");

    // Exit code must be nonzero — the operator needs a clear failure signal.
    assert!(
        !output.status.success(),
        "offline validate with a nonexistent config must exit nonzero, got: {:?}",
        output.status
    );

    // The supplied path must appear somewhere in stdout or stderr so the
    // operator can see which file could not be loaded.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // We match on the filename portion of the path, which is always present
    // and avoids any platform path-separator differences in the assertion.
    let filename = absent_path
        .file_name()
        .and_then(|n| n.to_str())
        .expect("temp filename should be valid UTF-8");

    assert!(
        combined.contains(filename) || combined.contains(path_str),
        "output must mention the supplied path so the operator knows which file is missing.\n\
         Expected to find: {path_str}\n\
         stdout: {stdout}\n\
         stderr: {stderr}"
    );
}
