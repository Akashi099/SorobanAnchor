#![cfg(feature = "std")]
//! CLI binary for AnchorKit.
//!
//! This binary is only available when building with the `std` feature (the default).
//! For WASM builds, disable default features:
//!   cargo build --target wasm32-unknown-unknown --no-default-features --features wasm

use clap::{Parser, Subcommand};
use serde::Serialize;

use anchorkit::normalize_stellar_account_id;
use anchorkit::config::{parse_runtime_config_str, ConfigFormat};

// ── SecretKey wrapper ─────────────────────────────────────────────────────────
//
// Wraps a Stellar secret key so it is never accidentally emitted to stdout,
// stderr, or debug output. Zeroizes key material on drop.

struct SecretKey(String);

impl SecretKey {
    fn new(raw: impl Into<String>) -> Self { Self(raw.into()) }
    fn expose(&self) -> &str { &self.0 }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::ops::Deref for SecretKey {
    type Target = str;
    fn deref(&self) -> &str { &self.0 }
}

impl AsRef<std::ffi::OsStr> for SecretKey {
    fn as_ref(&self) -> &std::ffi::OsStr { self.0.as_ref() }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

// ── Network profile management ────────────────────────────────────────────────

/// A custom network profile stored in `~/.anchorkit/networks.json`.
///
/// All three string fields are required and must be non-empty.  `horizon_url`
/// is optional.  `is_default` defaults to `false` when absent from JSON.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct NetworkProfile {
    name: String,
    rpc_url: String,
    network_passphrase: String,
    horizon_url: Option<String>,
    #[serde(default)]
    is_default: bool,
}

/// Errors that can arise when loading or validating network profiles.
#[derive(Debug, PartialEq)]
enum NetworkProfileError {
    /// The file could not be read from disk.
    IoError(String),
    /// The file content is not valid JSON.
    MalformedJson(String),
    /// A profile entry failed field-level validation.
    InvalidProfile { index: usize, reason: String },
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
        }
    }
}

/// Validate a single `NetworkProfile` entry.
///
/// Returns `Ok(())` when the profile is well-formed, or an error string
/// describing the first validation failure found.
fn validate_network_profile(profile: &NetworkProfile) -> Result<(), String> {
    if profile.name.trim().is_empty() {
        return Err("'name' must not be empty".to_string());
    }
    if profile.name.len() > 64 {
        return Err(format!("'name' is too long ({} chars, max 64)", profile.name.len()));
    }
    // Names must be URL-safe identifiers: alphanumeric, hyphens, underscores.
    if !profile.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!(
            "'name' contains invalid characters (only alphanumeric, '-', '_' allowed): '{}'",
            profile.name
        ));
    }
    if profile.rpc_url.trim().is_empty() {
        return Err("'rpc_url' must not be empty".to_string());
    }
    if !profile.rpc_url.starts_with("https://") && !profile.rpc_url.starts_with("http://") {
        return Err(format!(
            "'rpc_url' must start with 'https://' or 'http://': '{}'",
            profile.rpc_url
        ));
    }
    if profile.network_passphrase.trim().is_empty() {
        return Err("'network_passphrase' must not be empty".to_string());
    }
    if let Some(ref h) = profile.horizon_url {
        if !h.trim().is_empty()
            && !h.starts_with("https://")
            && !h.starts_with("http://")
        {
            return Err(format!(
                "'horizon_url' must start with 'https://' or 'http://': '{h}'"
            ));
        }
    }
    Ok(())
}

/// Load and validate network profiles from `networks_path()`.
///
/// Returns a tuple of:
/// - `Vec<NetworkProfile>`: all profiles that passed validation.
/// - `Vec<NetworkProfileError>`: every error encountered (file I/O, JSON parse,
///   or per-entry field validation).  Callers should surface these as warnings.
///
/// This function **never panics** and **never crashes the process**.  A missing
/// file is treated as an empty profile set (not an error).
fn load_network_profiles_with_diagnostics() -> (Vec<NetworkProfile>, Vec<NetworkProfileError>) {
    let path = networks_path();
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return (
                Vec::new(),
                vec![NetworkProfileError::IoError(e.to_string())],
            );
        }
    };

    // An empty file is treated as an empty profile set.
    if content.trim().is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Parse the top-level JSON value first so we can give a clear error for
    // completely malformed files before attempting typed deserialization.
    let raw_value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return (
                Vec::new(),
                vec![NetworkProfileError::MalformedJson(e.to_string())],
            );
        }
    };

    // The file must contain a JSON array at the top level.
    if !raw_value.is_array() {
        return (
            Vec::new(),
            vec![NetworkProfileError::MalformedJson(
                "expected a JSON array at the top level".to_string(),
            )],
        );
    }

    // Deserialize into typed structs.  Individual entries that fail to
    // deserialize are skipped with a diagnostic rather than aborting.
    let raw_array = raw_value.as_array().unwrap(); // safe: checked above
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
            Ok(profile) => {
                match validate_network_profile(&profile) {
                    Ok(()) => valid_profiles.push(profile),
                    Err(reason) => {
                        errors.push(NetworkProfileError::InvalidProfile { index, reason });
                    }
                }
            }
        }
    }

    (valid_profiles, errors)
}

fn networks_path() -> std::path::PathBuf {
    let dir = dirs_home().join(".anchorkit");
    std::fs::create_dir_all(&dir).ok();
    dir.join("networks.json")
}

fn dirs_home() -> std::path::PathBuf {
    // On Windows the home directory is USERPROFILE; fall back to HOME then ".".
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn secure_read_file(path: &str) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path)
}

fn load_network_profiles() -> Vec<NetworkProfile> {
    let (profiles, errors) = load_network_profiles_with_diagnostics();
    for err in &errors {
        eprintln!("warning: {err}");
    }
    profiles
}

fn save_network_profiles(profiles: &[NetworkProfile]) {
    let path = networks_path();
    let json = match serde_json::to_string_pretty(profiles) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("warning: could not serialize network profiles: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("warning: could not write {}: {e}", path.display());
    }
}

fn find_profile<'a>(profiles: &'a [NetworkProfile], name: &str) -> Option<&'a NetworkProfile> {
    profiles.iter().find(|p| p.name == name)
}

/// Built-in network names that are always available without a custom profile.
const BUILTIN_NETWORKS: &[&str] = &["testnet", "mainnet", "futurenet"];

/// Resolve the RPC URL for a network name.
///
/// Resolution order:
/// 1. Custom profile in `~/.anchorkit/networks.json`.
/// 2. Built-in network (testnet / mainnet / futurenet).
/// 3. Unknown network → falls back to testnet RPC with a clear warning.
fn rpc_url_for(network: &str) -> String {
    let profiles = load_network_profiles();
    if let Some(p) = find_profile(&profiles, network) {
        return p.rpc_url.clone();
    }
    if !BUILTIN_NETWORKS.contains(&network) {
        eprintln!(
            "warning: unknown network '{}' — no custom profile found. \
             Falling back to testnet RPC. \
             Add a profile with: anchorkit network add --name {network} --rpc-url <URL> --passphrase <PHRASE>",
            network
        );
    }
    rpc_url(network).to_string()
}

/// Resolve the network passphrase for a network name.
///
/// Resolution order mirrors [`rpc_url_for`].
fn passphrase_for(network: &str) -> String {
    let profiles = load_network_profiles();
    if let Some(p) = find_profile(&profiles, network) {
        return p.network_passphrase.clone();
    }
    if !BUILTIN_NETWORKS.contains(&network) {
        // Warning already emitted by rpc_url_for; avoid double-printing.
    }
    passphrase(network).to_string()
}

fn default_network() -> String {
    let profiles = load_network_profiles();
    profiles.iter()
        .find(|p| p.is_default)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "testnet".to_string())
}

/// Return the contract ID to use, checking the per-command arg first, then
/// the global flag / ANCHOR_CONTRACT_ID env var.  Exits with a clear error
/// if neither is set.
fn require_contract_id(global: Option<String>, local: Option<String>, command: &str) -> String {
    local.or(global).unwrap_or_else(|| {
        eprintln!("error: --contract-id (or ANCHOR_CONTRACT_ID) is required for `{command}`");
        eprintln!("hint:  pass --contract-id <ID>  or  export ANCHOR_CONTRACT_ID=<ID>");
        std::process::exit(1);
    })
}

/// Validate that `key` looks like a Stellar secret key (starts with 'S', non-empty).
/// Returns an error string when the key is invalid.
fn validate_stellar_secret(key: &str, source_label: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err(format!("{source_label}: signing key must not be empty"));
    }
    if !key.starts_with('S') {
        return Err(format!(
            "{source_label}: not a valid Stellar secret key (expected 'S...' format, got a key starting with '{}')",
            key.chars().next().unwrap_or('?')
        ));
    }
    Ok(())
}

/// Inner, infallible-return version of secret resolution used for unit testing.
///
/// Resolution order:
///   1. `ephemeral_token` (highest priority; one-time automated flow token)
///   2. `secret_key` flag
///   3. `ANCHOR_ADMIN_SECRET` environment variable
///   4. `keypair_file` (JSON `{"secret_key":"S..."}` or plain-text)
///   5. `credential_name` (keystore; requires interactive prompt)
///
/// Returns `Ok(raw_key_string)` on success or `Err(descriptive_message)` on failure.
fn try_resolve_source(
    ephemeral_token: Option<&str>,
    secret_key: Option<&str>,
    keypair_file: Option<&str>,
    credential_name: Option<&str>,
    no_interactive: bool,
    read_env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    // 1. Ephemeral token — highest priority, single-use automated token
    if let Some(tok) = ephemeral_token {
        if !tok.is_empty() {
            validate_stellar_secret(tok, "--ephemeral-token / ANCHORKIT_EPHEMERAL_TOKEN")?;
            return Ok(tok.to_string());
        }
    }

    // 2. Explicit --secret-key flag
    if let Some(sk) = secret_key {
        validate_stellar_secret(sk, "--secret-key")?;
        return Ok(sk.to_string());
    }

    // 3. ANCHOR_ADMIN_SECRET environment variable
    if let Some(sk) = read_env("ANCHOR_ADMIN_SECRET") {
        if sk.is_empty() {
            return Err(
                "ANCHOR_ADMIN_SECRET is set but empty — provide a valid Stellar secret key \
                 (expected 'S...' format) or unset the variable"
                    .to_string(),
            );
        }
        validate_stellar_secret(&sk, "ANCHOR_ADMIN_SECRET")?;
        return Ok(sk);
    }

    // 4. Keypair file
    if let Some(path) = keypair_file {
        let raw = secure_read_file(path)
            .map_err(|e| format!("cannot read keypair file '{path}': {e}"))?;
        let key = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            v.get("secret_key")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| raw.trim())
                .to_string()
        } else {
            raw.trim().to_string()
        };
        validate_stellar_secret(&key, &format!("keypair file '{path}'"))?;
        return Ok(key);
    }

    // 5. Keystore credential — requires interactive password prompt
    if let Some(name) = credential_name {
        if no_interactive {
            return Err(
                "--credential-name requires an interactive password prompt; \
                 use --secret-key, --ephemeral-token, or ANCHOR_ADMIN_SECRET in \
                 non-interactive mode"
                    .to_string(),
            );
        }
        // Actual keystore decryption happens in the caller (requires rpassword).
        return Err(format!("__keystore__{name}"));
    }

    Err("signing key required — provide one of:\n  \
         --secret-key <KEY>\n  \
         export ANCHOR_ADMIN_SECRET=<KEY>\n  \
         --keypair-file <PATH>\n  \
         --credential-name <NAME>  (use: anchorkit credentials add --name <NAME>)"
        .to_string())
}

/// Resolve the signing source from flags or environment.
/// Resolution order: ephemeral_token > --secret-key > ANCHOR_ADMIN_SECRET >
///                   --keypair-file > --credential-name
fn resolve_source(
    ephemeral_token: Option<&str>,
    secret_key: Option<&str>,
    keypair_file: Option<&str>,
    credential_name: Option<&str>,
    no_interactive: bool,
) -> SecretKey {
    match try_resolve_source(
        ephemeral_token,
        secret_key,
        keypair_file,
        credential_name,
        no_interactive,
        &|var| std::env::var(var).ok(),
    ) {
        Ok(key) => SecretKey::new(key),
        Err(msg) if msg.starts_with("__keystore__") => {
            let name = &msg["__keystore__".len()..];
            let password = rpassword::prompt_password("Keystore password: ")
                .unwrap_or_else(|e| { eprintln!("error: failed to read password: {e}"); std::process::exit(1); });
            keystore_get_decrypted(name, &password)
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    }
}

fn normalize_stellar_public_address(field: &str, address: &str) -> String {
    match normalize_stellar_account_id(address) {
        Ok(normalized) => normalized,
        Err(err) => {
            eprintln!("error: invalid {field}: {0}", err.message);
            std::process::exit(1);
        }
    }
}

// ── RPC helpers ───────────────────────────────────────────────────────────────

fn rpc_url(network: &str) -> &'static str {
    match network {
        "mainnet"   => "https://horizon.stellar.org",
        "futurenet" => "https://rpc-futurenet.stellar.org",
        _           => "https://soroban-testnet.stellar.org",
    }
}

fn passphrase(network: &str) -> &'static str {
    match network {
        "mainnet"   => "Public Global Stellar Network ; September 2015",
        "futurenet" => "Test SDF Future Network ; October 2022",
        _           => "Test SDF Network ; September 2015",
    }
}

fn stellar_invoke(
    contract_id: &str,
    // SECURITY: `source` is a Stellar secret key passed to the Stellar CLI via
    // `--source`.  It is intentionally exposed here because the upstream CLI
    // requires it as a positional argument.  It must never be echoed to stdout
    // or included in log messages; only the exit status and stdout of the child
    // process are surfaced to the caller.
    source: &SecretKey,
    network: &str,
    fn_args: &[&str],
) -> String {
    let url = rpc_url_for(network);
    let phrase = passphrase_for(network);
    let source: &str = source; // coerce &SecretKey → &str for uniform array element type
    let output = std::process::Command::new("stellar")
        .args(["contract", "invoke",
               "--id", contract_id,
               "--source", source,
               "--rpc-url", &url,
               "--network-passphrase", &phrase,
               "--"])
        .args(fn_args)
        .output()
        .unwrap_or_else(|e| { eprintln!("error: failed to run stellar contract invoke — is the Stellar CLI installed? ({e})"); std::process::exit(1); });

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        // Emit only the child's stderr; the secret key is not present there.
        eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim());
        std::process::exit(1);
    }
}

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "anchorkit", about = "SorobanAnchor CLI")]
struct Cli {
    /// Contract ID to invoke (or set ANCHOR_CONTRACT_ID)
    #[arg(long, global = true, env = "ANCHOR_CONTRACT_ID")]
    contract_id: Option<String>,

    /// Stellar network: testnet | mainnet | futurenet | <custom> (or set STELLAR_NETWORK)
    #[arg(long, global = true, env = "STELLAR_NETWORK")]
    network: Option<String>,

    /// Disable all interactive prompts; batch scripts use this to avoid hanging on input.
    /// Also enabled by setting ANCHORKIT_NO_INTERACTIVE=1.
    #[arg(long, global = true, env = "ANCHORKIT_NO_INTERACTIVE")]
    no_interactive: bool,

    /// One-time ephemeral signing token (highest priority over other key sources; zeroized after use).
    /// Intended for single-operation authorization in automated flows.
    /// Also settable via ANCHORKIT_EPHEMERAL_TOKEN.
    #[arg(long, global = true, env = "ANCHORKIT_EPHEMERAL_TOKEN")]
    ephemeral_token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy contract to a network
    Deploy {
        #[arg(long, default_value = "testnet")]
        network: String,
        #[arg(long, default_value = "default")]
        source: String,
        /// Admin address for post-deployment initialization
        #[arg(long)]
        admin: Option<String>,
        /// Validate without deploying
        #[arg(long)]
        dry_run: bool,
        /// List deployment history
        #[arg(long)]
        list: bool,
        /// Upgrade an existing contract instead of deploying a new one.
        /// Requires --contract-id (or ANCHOR_CONTRACT_ID) and --secret-key / ANCHOR_ADMIN_SECRET.
        #[arg(long)]
        upgrade: bool,
        /// Secret key used to sign the upgrade transaction (overrides ANCHOR_ADMIN_SECRET)
        #[arg(long)]
        secret_key: Option<String>,
        /// Path to a JSON or plain-text keypair file (used when --secret-key is absent)
        #[arg(long)]
        keypair_file: Option<String>,
        /// Skip the interactive mainnet confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Register an attestor
    Register {
        #[arg(long)] address: String,
        #[arg(long, value_delimiter = ',')] services: Vec<String>,
        #[arg(long)] contract_id: Option<String>,
        #[arg(long, default_value = "testnet")] network: String,
        #[arg(long)] secret_key: Option<String>,
        #[arg(long)] keypair_file: Option<String>,
        /// Name of a credential stored in the keystore (alternative to --secret-key)
        #[arg(long)] credential_name: Option<String>,
        #[arg(long)] sep10_token: String,
        #[arg(long)] sep10_issuer: String,
    },
    /// Submit an attestation
    Attest {
        #[arg(long)] subject: String,
        #[arg(long)] payload_hash: String,
        #[arg(long)] contract_id: Option<String>,
        #[arg(long, default_value = "testnet")] network: String,
        #[arg(long)] secret_key: Option<String>,
        #[arg(long)] keypair_file: Option<String>,
        /// Name of a credential stored in the keystore (alternative to --secret-key)
        #[arg(long)] credential_name: Option<String>,
        #[arg(long)] issuer: String,
        #[arg(long)] session_id: Option<u64>,
        /// Ed25519 secret key (Stellar 'S...' format) used to sign the payload.
        /// If omitted, the transaction source key (--secret-key / --keypair-file) is used.
        #[arg(long)] signing_key: Option<String>,
    },
    /// Get the best quote for a currency pair
    Quote {
        /// Source asset code (e.g. USDC)
        #[arg(long)] from: String,
        /// Destination asset code (e.g. XLM)
        #[arg(long)] to: String,
        /// Amount in base asset units
        #[arg(long)] amount: u64,
        #[arg(long)] contract_id: Option<String>,
        #[arg(long, default_value = "testnet")] network: String,
        #[arg(long)] secret_key: Option<String>,
        #[arg(long)] keypair_file: Option<String>,
        /// Name of a credential stored in the keystore (alternative to --secret-key)
        #[arg(long)] credential_name: Option<String>,
    },
    /// Fetch SEP-6 transaction status from an anchor URL
    Status {
        /// Transaction ID to look up
        #[arg(long)] tx_id: String,
        /// Anchor base URL (e.g. https://anchor.example.com)
        #[arg(long)] anchor_url: String,
        /// Optional HTTP proxy URL for the request (e.g. http://proxy.corp.example.com:3128)
        #[arg(long)] proxy_url: Option<String>,
        /// Comma-separated list of hosts that bypass the proxy (e.g. localhost,127.0.0.1)
        #[arg(long)] no_proxy: Option<String>,
    },
    /// Revoke an attestor
    Revoke {
        #[arg(long)] address: String,
        #[arg(long)] contract_id: Option<String>,
        #[arg(long, default_value = "testnet")] network: String,
        #[arg(long)] secret_key: Option<String>,
        #[arg(long)] keypair_file: Option<String>,
        /// Name of a credential stored in the keystore (alternative to --secret-key)
        #[arg(long)] credential_name: Option<String>,
    },
    /// Manage stored credentials (encrypted secret keys)
    Credentials {
        #[command(subcommand)]
        action: CredentialsAction,
    },
    /// Check environment setup
    Doctor {
        /// Attempt to automatically fix issues
        #[arg(long)]
        fix: bool,
    },
    /// Query contract health, metadata freshness, and rate limiter status
    Health {
        /// Contract ID to query (or set ANCHOR_CONTRACT_ID)
        #[arg(long)]
        contract_id: String,
        #[arg(long, default_value = "testnet")]
        network: String,
        #[arg(long)]
        secret_key: Option<String>,
        #[arg(long)]
        keypair_file: Option<String>,
        /// Anchor address to check metadata freshness for (optional)
        #[arg(long)]
        anchor: Option<String>,
        /// Attestor address to check rate limiter health for (optional)
        #[arg(long)]
        attestor: Option<String>,
    },
    /// Manage custom network profiles
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// Fetch and display a stellar.toml from an anchor domain
    Discover {
        /// Anchor base URL (e.g. https://anchor.example.com)
        #[arg(long)] anchor_url: String,
        /// Optional HTTP proxy URL (e.g. http://proxy.corp.example.com:3128)
        #[arg(long)] proxy_url: Option<String>,
        /// Comma-separated no-proxy bypass list (e.g. localhost,127.0.0.1)
        #[arg(long)] no_proxy: Option<String>,
        /// Request timeout in seconds (default: 30)
        #[arg(long, default_value = "30")] timeout: u64,
    },
    /// Offline operations (config validation, workflow simulation) — no network required
    Offline {
        #[command(subcommand)]
        action: OfflineAction,
    },
    /// Verify an on-chain attestation by ID or payload hash
    Verify {
        /// Attestation ID (mutually exclusive with --payload-hash)
        #[arg(long)]
        id: Option<u64>,
        /// Payload hash to look up (mutually exclusive with --id)
        #[arg(long)]
        payload_hash: Option<String>,
        /// Local file whose SHA-256 hash is compared against the stored hash
        #[arg(long)]
        payload_file: Option<String>,
        /// Contract ID (overrides --contract-id / ANCHOR_CONTRACT_ID)
        #[arg(long)]
        contract_id: Option<String>,
        #[arg(long, default_value = "testnet")]
        network: String,
        #[arg(long)]
        secret_key: Option<String>,
        #[arg(long)]
        keypair_file: Option<String>,
    },
}

#[derive(Subcommand)]
enum OfflineAction {
    /// Validate config files without network access
    Validate {
        /// Path to a specific config file (validates all in configs/ when omitted)
        #[arg(long)] config: Option<String>,
    },
    /// Simulate a named workflow without network access
    Simulate {
        /// Path to a config file (uses configs/ when omitted)
        #[arg(long)] config: Option<String>,
        /// Workflow name: deploy | register | attest
        #[arg(long)] workflow: String,
    },
}

#[derive(Subcommand)]
enum NetworkAction {
    /// Add a custom network profile
    Add {
        #[arg(long)] name: String,
        #[arg(long)] rpc_url: String,
        #[arg(long)] passphrase: String,
        #[arg(long)] horizon_url: Option<String>,
    },
    /// List all configured network profiles
    List,
    /// Remove a custom network profile
    Remove {
        #[arg(long)] name: String,
    },
    /// Set the default network
    SetDefault {
        #[arg(long)] name: String,
    },
}

#[derive(Subcommand)]
enum CredentialsAction {
    /// Store an encrypted credential
    Add {
        #[arg(long)] name: String,
        /// Secret key value (prompted if omitted)
        #[arg(long)] value: Option<String>,
    },
    /// Retrieve and print a stored credential
    Get {
        #[arg(long)] name: String,
    },
    /// List all stored credential names
    List,
    /// Remove a stored credential
    Remove {
        #[arg(long)] name: String,
    },
    /// Rotate the keystore password, re-encrypting all stored credentials
    Rotate,
}

// ── Output types (JSON) ───────────────────────────────────────────────────────

#[derive(Serialize, serde::Deserialize)]
struct QuoteOutput {
    quote_id: u64,
    anchor: String,
    base_asset: String,
    quote_asset: String,
    rate: u64,
    fee_percentage: u32,
    minimum_amount: u64,
    maximum_amount: u64,
    valid_until: u64,
}

#[derive(Serialize)]
struct StatusOutput {
    transaction_id: String,
    kind: String,
    status: String,
    amount_in: Option<u64>,
    amount_out: Option<u64>,
    amount_fee: Option<u64>,
    message: Option<String>,
}

// ── Command implementations ───────────────────────────────────────────────────

// ── Deployments record ────────────────────────────────────────────────────────

#[derive(Serialize, serde::Deserialize, Clone)]
struct DeploymentRecord {
    contract_id: String,
    network: String,
    timestamp: u64,
    initialized: bool,
}

fn deployments_path() -> std::path::PathBuf {
    let dir = std::path::Path::new(".anchorkit");
    std::fs::create_dir_all(dir).ok();
    dir.join("deployments.json")
}

fn load_deployments() -> Vec<DeploymentRecord> {
    let path = deployments_path();
    if !path.exists() { return Vec::new(); }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_deployments(records: &[DeploymentRecord]) {
    let path = deployments_path();
    let json = serde_json::to_string_pretty(records).unwrap_or_default();
    std::fs::write(path, json).ok();
}

// ── Pre-deployment validation ─────────────────────────────────────────────────

fn pre_deploy_validate(network: &str) -> bool {
    let mut ok = true;

    // 1. WASM artifact exists
    let wasm = "target/wasm32-unknown-unknown/release/anchorkit.wasm";
    if std::path::Path::new(wasm).exists() {
        println!("  ✓ WASM artifact found");
    } else {
        eprintln!("  ✗ WASM not found at {wasm} — run: cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm");
        ok = false;
    }

    // 2. Config files valid
    let config_check = check_config_files();
    if config_check.passed {
        println!("  ✓ Config files valid");
    } else {
        eprintln!("  ✗ {}", config_check.message);
        ok = false;
    }

    // 3. Network reachable
    let net_check = check_network_connectivity(network);
    if net_check.passed {
        println!("  ✓ Network reachable");
    } else {
        eprintln!("  ✗ {}", net_check.message);
        ok = false;
    }

    ok
}

/// Upgrade an existing contract to a freshly-built WASM.
///
/// Steps:
///   1. Build the WASM artifact.
///   2. Upload the WASM to the network and obtain its hash.
///   3. Call `upgrade(new_wasm_hash)` on the contract.
///   4. Call `migrate()` to apply any state-schema changes.
fn upgrade_contract(contract_id: &str, network: &str, source: &SecretKey) {
    println!("\n🔍 Pre-upgrade validation ({network})...");
    if !pre_deploy_validate(network) {
        eprintln!("\n❌ Pre-upgrade validation failed. Aborting.");
        std::process::exit(1);
    }
    println!("✅ Validation passed.\n");

    // Build WASM.
    println!("Building WASM...");
    let build = std::process::Command::new("cargo")
        .args([
            "build", "--release",
            "--target", "wasm32-unknown-unknown",
            "--no-default-features", "--features", "wasm",
        ])
        .status()
        .unwrap_or_else(|e| { eprintln!("error: failed to run cargo build: {e}"); std::process::exit(1); });
    if !build.success() {
        eprintln!("WASM build failed");
        std::process::exit(1);
    }

    let wasm = "target/wasm32-unknown-unknown/release/anchorkit.wasm";
    let net_url = rpc_url_for(network);
    let net_phrase = passphrase_for(network);

    // Upload WASM and capture the resulting hash.
    println!("Uploading WASM to {network}...");
    let source_str: &str = source; // coerce &SecretKey → &str for uniform array element type
    let upload_output = std::process::Command::new("stellar")
        .args([
            "contract", "upload",
            "--wasm", wasm,
            "--source", source_str,
            "--rpc-url", &net_url,
            "--network-passphrase", &net_phrase,
        ])
        .output()
        .unwrap_or_else(|e| { eprintln!("error: failed to run stellar contract upload — is the Stellar CLI installed? ({e})"); std::process::exit(1); });

    if !upload_output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&upload_output.stderr).trim());
        std::process::exit(1);
    }

    let new_wasm_hash = String::from_utf8_lossy(&upload_output.stdout).trim().to_string();
    println!("New WASM hash: {new_wasm_hash}");

    // Call upgrade() on the contract.
    println!("Calling upgrade() on contract {contract_id}...");
    stellar_invoke(contract_id, source, network, &[
        "upgrade",
        "--new_wasm_hash", &new_wasm_hash,
    ]);

    // Call migrate() to apply state-schema changes.
    // Pass new_schema_version = 1 for the initial migration after upgrade.
    println!("Calling migrate() on contract {contract_id}...");
    stellar_invoke(contract_id, source, network, &["migrate", "--new_schema_version", "1"]);

    println!("✅ Contract upgraded successfully.");
    println!("   Contract ID : {contract_id}");
    println!("   New WASM    : {new_wasm_hash}");
}

/// Validate an optional `--admin` argument for `deploy`.
///
/// `None` (no admin flag) and the literal alias `"default"` are always
/// accepted; any other value must be a well-formed Stellar public address
/// (`G...`). Kept as a pure function (no process exit) so it is unit-testable.
fn validate_admin_arg(admin: Option<&str>) -> Result<(), String> {
    match admin {
        None | Some("default") => Ok(()),
        Some(addr) => normalize_stellar_account_id(addr)
            .map(|_| ())
            .map_err(|err| format!("invalid --admin address '{addr}': {}", err.message)),
    }
}

/// Validate that `--services` names at least one known service.
/// Kept as a pure function (no process exit) so it is unit-testable.
fn validate_services_arg(services: &[String]) -> Result<(), String> {
    if services.is_empty() {
        return Err(
            "--services must name at least one service: deposits, withdrawals, quotes, kyc"
                .to_string(),
        );
    }
    Ok(())
}

/// Prompt the operator to confirm a mainnet deployment.
///
/// Returns `true` when the deployment should proceed: either the user typed
/// `y`/`yes`, or the prompt was skipped via `--yes` / `--no-interactive`.
fn confirm_mainnet_deploy(network: &str, yes: bool, no_interactive: bool) -> bool {
    if network != "mainnet" || yes || no_interactive {
        return true;
    }
    eprint!("⚠️  You are about to deploy to MAINNET. Continue? [y/N]: ");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn deploy(network: &str, source: &str, admin: Option<&str>, dry_run: bool, list: bool, yes: bool, no_interactive: bool) {
    // --list: print deployment history and exit
    if list {
        let records = load_deployments();
        if records.is_empty() {
            println!("No deployments recorded.");
        } else {
            println!("{}", serde_json::to_string_pretty(&records).unwrap_or_default());
        }
        return;
    }

    if let Err(e) = validate_admin_arg(admin) {
        eprintln!("error: {e}");
        eprintln!("hint: pass --admin <STELLAR_PUBLIC_ADDRESS> (starts with 'G'), or omit --admin to use the source key");
        std::process::exit(1);
    }

    if !dry_run && !confirm_mainnet_deploy(network, yes, no_interactive) {
        eprintln!("Aborted: mainnet deployment not confirmed.");
        eprintln!("hint: re-run with --yes to skip this prompt in scripted/CI environments.");
        std::process::exit(1);
    }

    println!("\n🔍 Pre-deployment validation ({network})...");
    if !pre_deploy_validate(network) {
        eprintln!("\n❌ Pre-deployment validation failed. Aborting.");
        std::process::exit(1);
    }
    println!("✅ Validation passed.\n");

    if dry_run {
        println!("--dry-run: skipping actual deployment.");
        return;
    }

    // Build WASM
    println!("Building WASM...");
    let build = std::process::Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown",
               "--no-default-features", "--features", "wasm"])
        .status()
        .unwrap_or_else(|e| { eprintln!("error: failed to run cargo build: {e}"); std::process::exit(1); });
    if !build.success() { eprintln!("WASM build failed"); std::process::exit(1); }

    let wasm = "target/wasm32-unknown-unknown/release/anchorkit.wasm";
    println!("Deploying {wasm} to {network}...");
    let net_url = rpc_url_for(network);
    let net_phrase = passphrase_for(network);
    let output = std::process::Command::new("stellar")
        .args(["contract", "deploy", "--wasm", wasm,
               // SECURITY: `source` is a Stellar secret key required by the
               // Stellar CLI.  It is passed only as a subprocess argument and
               // is never echoed to stdout or included in log messages.
               "--source", source,
               "--rpc-url", &net_url,
               "--network-passphrase", &net_phrase])
        .output()
        .unwrap_or_else(|e| { eprintln!("error: failed to run stellar contract deploy — is the Stellar CLI installed? ({e})"); std::process::exit(1); });

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim());
        std::process::exit(1);
    }

    let contract_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("Contract ID: {contract_id}");

    // Save to deployments.json
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut records = load_deployments();
    let mut record = DeploymentRecord {
        contract_id: contract_id.clone(),
        network: network.to_string(),
        timestamp,
        initialized: false,
    };

    // Post-deployment initialization.
    // `admin_addr` is a Stellar *public* address (G...) or the alias "default".
    // If the caller omitted --admin, we fall back to the source identifier
    // (which may be a key alias, not the raw secret).  We print only the
    // admin address, never the signing key.
    let admin_addr = admin.unwrap_or("default");
    println!("Initializing contract with admin {admin_addr}...");
    let init_result = std::process::Command::new("stellar")
        .args(["contract", "invoke",
               "--id", &contract_id,
               // SECURITY: `source` passed only as subprocess arg, not logged.
               "--source", source,
               "--rpc-url", &net_url,
               "--network-passphrase", &net_phrase,
               "--", "initialize",
               "--admin", admin_addr])
        .output();

    match init_result {
        Ok(out) if out.status.success() => {
            println!("✅ Contract initialized.");
            record.initialized = true;
        }
        Ok(out) => {
            eprintln!("⚠️  Post-deployment initialization failed:");
            eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim());
            eprintln!("\nContract ID: {contract_id}");
            eprintln!("To initialize manually: stellar contract invoke --id {contract_id} --source <SIGNING_KEY_OR_ALIAS> -- initialize --admin <ADMIN_ADDRESS>");
        }
        Err(e) => {
            eprintln!("⚠️  Could not run initialization: {e}");
            eprintln!("Contract ID: {contract_id}");
        }
    }

    records.push(record);
    save_deployments(&records);
    println!("Deployment saved to .anchorkit/deployments.json");
}

fn parse_services(services: &[String]) -> Vec<u32> {
    services.iter().map(|s| match s.trim() {
        "deposits"    => 1,
        "withdrawals" => 2,
        "quotes"      => 3,
        "kyc"         => 4,
        other => { eprintln!("Unknown service: {other}"); std::process::exit(1); }
    }).collect()
}

fn derive_ed25519_public_key_hex(source: &str) -> String {
    use stellar_strkey::Strkey;
    let strkey = Strkey::from_string(source)
        .unwrap_or_else(|e| { eprintln!("error: invalid secret key '{}': {e}", source); std::process::exit(1); });
    let seed = match strkey {
        Strkey::PrivateKeyEd25519(k) => k.0,
        _ => { eprintln!("error: expected an Ed25519 secret key (starts with 'S')"); std::process::exit(1); }
    };
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    signing_key.verifying_key().as_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode a lowercase hex string to bytes, returning an error on invalid input.
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex string".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Produce a hex-encoded Ed25519 signature over `payload_hash` using `secret_key_str`.
///
/// `payload_hash` is decoded from hex if it is a valid even-length hex string;
/// otherwise its UTF-8 bytes are signed directly.
fn compute_ed25519_signature_hex(secret_key_str: &str, payload_hash: &str) -> String {
    use stellar_strkey::Strkey;
    use ed25519_dalek::Signer;
    let strkey = Strkey::from_string(secret_key_str)
        .unwrap_or_else(|e| { eprintln!("error: invalid signing key: {e}"); std::process::exit(1); });
    let seed = match strkey {
        Strkey::PrivateKeyEd25519(k) => k.0,
        _ => { eprintln!("error: signing key must be an Ed25519 secret key (starts with 'S')"); std::process::exit(1); }
    };
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let payload_bytes = decode_hex(payload_hash)
        .unwrap_or_else(|_| payload_hash.as_bytes().to_vec());
    let signature = signing_key.sign(&payload_bytes);
    signature.to_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

fn register(
    address: &str, services: &[String], contract_id: &str,
    network: &str, source: &SecretKey, sep10_token: &str, sep10_issuer: &str,
) {
    if let Err(e) = validate_services_arg(services) {
        eprintln!("error: {e}");
        eprintln!("hint: anchorkit register --address <ADDR> --services deposits,withdrawals ...");
        std::process::exit(1);
    }
    let address = normalize_stellar_public_address("attestor address", address);
    let sep10_issuer = normalize_stellar_public_address("SEP-10 issuer address", sep10_issuer);
    let service_ids = parse_services(services)
        .iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");

    let pk_hex = derive_ed25519_public_key_hex(source);
    stellar_invoke(contract_id, source, network, &[
        "register_attestor",
        "--attestor", &address,
        "--sep10_token", sep10_token,
        "--sep10_issuer", &sep10_issuer,
        "--public_key", &pk_hex,
    ]);
    stellar_invoke(contract_id, source, network, &[
        "configure_services",
        "--anchor", &address,
        "--services", &service_ids,
    ]);
    println!("Attestor {address} registered and services configured.");
}

fn attest(
    subject: &str, payload_hash: &str, contract_id: &str,
    network: &str, source: &SecretKey, issuer: &str, session_id: Option<u64>,
    signing_key: Option<&str>,
) {
    let subject = normalize_stellar_public_address("subject address", subject);
    let issuer = normalize_stellar_public_address("issuer address", issuer);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().to_string();

    // Use the dedicated signing key when provided; fall back to the transaction source key,
    // which is the same key registered via `anchorkit register --public-key`.
    let key_str = signing_key.unwrap_or_else(|| source.expose());
    let signature = compute_ed25519_signature_hex(key_str, payload_hash);

    let session_str;
    let result = if let Some(sid) = session_id {
        session_str = sid.to_string();
        stellar_invoke(contract_id, source, network, &[
            "submit_attestation_with_session",
            "--session_id", &session_str,
            "--issuer", &issuer, "--subject", &subject,
            "--timestamp", &timestamp,
            "--payload_hash", payload_hash,
            "--signature", &signature,
        ])
    } else {
        stellar_invoke(contract_id, source, network, &[
            "submit_attestation",
            "--issuer", &issuer, "--subject", &subject,
            "--timestamp", &timestamp,
            "--payload_hash", payload_hash,
            "--signature", &signature,
        ])
    };
    println!("Attestation ID: {result}");
}

fn quote(from: &str, to: &str, amount: u64, contract_id: &str, network: &str, source: &SecretKey) {
    let amount_str = amount.to_string();
    // route_transaction takes a RoutingOptions XDR; pass individual fields via stellar CLI args
    let raw = stellar_invoke(contract_id, source, network, &[
        "route_transaction",
        "--base_asset", from,
        "--quote_asset", to,
        "--amount", &amount_str,
        "--operation_type", "1",   // 1 = deposit
        "--strategy", "LowestFee",
        "--min_reputation", "0",
        "--max_anchors", "10",
        "--require_kyc", "false",
    ]);

    // The stellar CLI returns XDR or JSON; parse as JSON first, fall back to raw print
    let out: QuoteOutput = serde_json::from_str(&raw).unwrap_or_else(|_| {
        // stellar CLI may return a plain contract-encoded value; surface it as-is
        eprintln!("note: could not parse quote as JSON, raw output:\n{raw}");
        std::process::exit(1);
    });
    match serde_json::to_string_pretty(&out) {
        Ok(s) => println!("{s}"),
        Err(e) => { eprintln!("error: failed to serialize quote output: {e}"); std::process::exit(1); }
    }
}

fn status(tx_id: &str, anchor_url: &str, proxy_url: Option<&str>, no_proxy: Option<&str>) {
    let url = format!("{}/sep6/transaction?id={}", anchor_url.trim_end_matches('/'), tx_id);

    // Build a proxy-aware client.
    let proxy_cfg = anchorkit::ProxyConfig {
        proxy_url: proxy_url.map(|s| s.to_string()),
        no_proxy: no_proxy.map(|s| s.to_string()),
        ..anchorkit::ProxyConfig::default()
    };
    let client = anchorkit::build_client(
        if proxy_cfg.is_configured() { Some(&proxy_cfg) } else { None },
        30,
    )
    .unwrap_or_else(|e| { eprintln!("error: failed to build HTTP client: {e}"); std::process::exit(1); });

    let resp = client
        .get(&url)
        .send()
        .unwrap_or_else(|e| { eprintln!("error: request failed: {e}"); std::process::exit(1); });

    if !resp.status().is_success() {
        eprintln!("error: anchor returned HTTP {}", resp.status());
        std::process::exit(1);
    }

    let body: serde_json::Value = resp.json()
        .unwrap_or_else(|e| { eprintln!("error: invalid JSON from anchor: {e}"); std::process::exit(1); });

    // SEP-6 wraps the transaction under a "transaction" key
    let tx = body.get("transaction").unwrap_or(&body);

    let kind = tx.get("kind").and_then(|v| v.as_str()).unwrap_or("deposit").to_string();
    let out = StatusOutput {
        transaction_id: tx.get("id").and_then(|v| v.as_str()).unwrap_or(tx_id).to_string(),
        kind,
        status: tx.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
        amount_in:  tx.get("amount_in").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
        amount_out: tx.get("amount_out").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
        amount_fee: tx.get("amount_fee").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
        message:    tx.get("message").and_then(|v| v.as_str()).map(|s| s.to_string()),
    };
    match serde_json::to_string_pretty(&out) {
        Ok(s) => println!("{s}"),
        Err(e) => { eprintln!("error: failed to serialize status output: {e}"); std::process::exit(1); }
    }
}

fn revoke(address: &str, contract_id: &str, network: &str, source: &SecretKey) {
    stellar_invoke(contract_id, source, network, &[
        "revoke_attestor",
        "--attestor", &address,
    ]);
    println!("{{\"revoked\": true, \"address\": \"{address}\"}}");
}

// ── Doctor command ────────────────────────────────────────────────────────────

struct CheckResult {
    passed: bool,
    warning: bool,
    message: String,
}

impl CheckResult {
    fn pass(msg: impl Into<String>) -> Self {
        Self { passed: true, warning: false, message: msg.into() }
    }
    fn fail(msg: impl Into<String>) -> Self {
        Self { passed: false, warning: false, message: msg.into() }
    }
    fn warn(msg: impl Into<String>) -> Self {
        Self { passed: true, warning: true, message: msg.into() }
    }
    fn icon(&self) -> &str {
        if !self.passed { "✗" } else if self.warning { "⚠" } else { "✓" }
    }
    fn color(&self) -> &str {
        if !self.passed { "\x1b[31m" } else if self.warning { "\x1b[33m" } else { "\x1b[32m" }
    }
}

fn check_stellar_cli() -> CheckResult {
    match std::process::Command::new("stellar").arg("--version").output() {
        Ok(output) => {
            let version_str = String::from_utf8_lossy(&output.stdout);
            if let Some(version_line) = version_str.lines().next() {
                // Parse version like "stellar 21.0.0"
                if let Some(ver) = version_line.split_whitespace().nth(1) {
                    if let Some(major) = ver.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
                        if major >= 21 {
                            return CheckResult::pass(format!("Stellar CLI {} installed", ver));
                        } else {
                            return CheckResult::fail(format!("Stellar CLI {} found, but v21+ required", ver));
                        }
                    }
                }
            }
            CheckResult::warn("Stellar CLI installed but version could not be parsed")
        }
        Err(_) => CheckResult::fail("Stellar CLI not found in PATH"),
    }
}

fn check_wasm_target(fix: bool) -> CheckResult {
    let output = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    
    match output {
        Ok(out) => {
            let targets = String::from_utf8_lossy(&out.stdout);
            if targets.contains("wasm32-unknown-unknown") {
                CheckResult::pass("wasm32-unknown-unknown target installed")
            } else if fix {
                println!("  Attempting to install wasm32-unknown-unknown...");
                let install = std::process::Command::new("rustup")
                    .args(["target", "add", "wasm32-unknown-unknown"])
                    .status();
                if install.map(|s| s.success()).unwrap_or(false) {
                    CheckResult::pass("wasm32-unknown-unknown target installed (auto-fixed)")
                } else {
                    CheckResult::fail("wasm32-unknown-unknown target missing and auto-fix failed")
                }
            } else {
                CheckResult::fail("wasm32-unknown-unknown target not installed (run: rustup target add wasm32-unknown-unknown)")
            }
        }
        Err(_) => CheckResult::fail("rustup not found"),
    }
}

fn check_contract_id_env() -> CheckResult {
    match std::env::var("ANCHOR_CONTRACT_ID") {
        Ok(id) if !id.is_empty() => CheckResult::pass(format!("ANCHOR_CONTRACT_ID set: {}", &id[..id.len().min(16)])),
        _ => CheckResult::warn("ANCHOR_CONTRACT_ID not set (required for contract operations)"),
    }
}

fn check_admin_secret_env() -> CheckResult {
    match std::env::var("ANCHOR_ADMIN_SECRET") {
        Ok(secret) if !secret.is_empty() && secret.starts_with('S') => {
            // Confirm presence and basic format only — never log the value.
            CheckResult::pass("ANCHOR_ADMIN_SECRET set and appears valid (starts with 'S')")
        }
        Ok(secret) if !secret.is_empty() => {
            // Value present but does not look like a Stellar secret key.
            // Do NOT include the value or any prefix in the message.
            CheckResult::fail("ANCHOR_ADMIN_SECRET is set but does not appear to be a valid Stellar secret key (expected 'S...' format)")
        }
        Ok(_) => CheckResult::warn("ANCHOR_ADMIN_SECRET is set but empty"),
        Err(_) => CheckResult::warn("ANCHOR_ADMIN_SECRET not set (required for signing operations)"),
    }
}

fn check_network_connectivity(network: &str) -> CheckResult {
    let url = rpc_url_for(network);
    check_network_connectivity_url(&url)
}

fn check_contract_deployment(contract_id: &str, network: &str) -> CheckResult {
    // Use the SecretKey wrapper so the value is never accidentally logged.
    // Fall back to the "default" alias (a named key in the Stellar CLI keystore)
    // rather than embedding a raw secret in the subprocess arguments.
    let source = std::env::var("ANCHOR_ADMIN_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .map(SecretKey::new)
        .unwrap_or_else(|| SecretKey::new("default"));

    let source_str: &str = &*source; // coerce SecretKey → &str for uniform array element type
    let output = std::process::Command::new("stellar")
        .args(["contract", "invoke",
               "--id", contract_id,
               "--source", source_str,
               "--rpc-url", &rpc_url_for(network),
               "--network-passphrase", &passphrase_for(network),
               "--",
               "is_initialized"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            CheckResult::pass(format!("Contract {} is deployed and responding", &contract_id[..contract_id.len().min(16)]))
        }
        Ok(_) => CheckResult::fail("Contract exists but failed to respond (may not be initialized)"),
        Err(_) => CheckResult::fail("Failed to query contract"),
    }
}

fn check_config_files() -> CheckResult {
    check_config_files_in(std::path::Path::new("configs"))
}

/// Validate every `.json`/`.toml` file in `config_dir` against the full
/// `RuntimeConfig` schema (not just syntactic parseability — see #634).
/// Split out from [`check_config_files`] so tests can point it at a
/// scratch directory instead of the repo's real `configs/`.
fn check_config_files_in(config_dir: &std::path::Path) -> CheckResult {
    if !config_dir.exists() {
        return CheckResult::warn("configs/ directory not found");
    }

    let mut valid_count = 0;
    let mut total_count = 0;
    let mut failures: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(config_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(ext) if ext == "json" || ext == "toml" => ext.to_string(),
                _ => continue,
            };
            total_count += 1;
            let label = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{label}: {e}"));
                    continue;
                }
            };
            let format = if ext == "json" { ConfigFormat::Json } else { ConfigFormat::Toml };
            match parse_runtime_config_str(&content, format) {
                Ok(_) => valid_count += 1,
                Err(e) => failures.push(format!("{label}: {e}")),
            }
        }
    }

    if total_count == 0 {
        CheckResult::warn("No config files found in configs/")
    } else if valid_count == total_count {
        CheckResult::pass(format!("{} config file(s) validated against schema", total_count))
    } else {
        CheckResult::fail(format!(
            "{}/{} config files are schema-valid — {}",
            valid_count, total_count, failures.join("; ")
        ))
    }
}

/// Verify the Soroban WASM contract has been built at least once, in either
/// profile. Missing artifacts are reported as a warning (not everyone running
/// `doctor` needs a contract build — e.g. someone only using the off-chain
/// SEP clients) with an actionable build command.
fn check_build_artifacts() -> CheckResult {
    check_build_artifacts_at(std::path::Path::new("."))
}

fn check_build_artifacts_at(root: &std::path::Path) -> CheckResult {
    let target_dir = root.join("target/wasm32-unknown-unknown");
    if !target_dir.exists() {
        return CheckResult::warn(
            "No WASM build artifacts found (run: cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm)"
        );
    }

    for profile in ["release", "debug"] {
        let profile_dir = target_dir.join(profile);
        if let Ok(entries) = std::fs::read_dir(&profile_dir) {
            let wasm_files: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
                .collect();
            if !wasm_files.is_empty() {
                return CheckResult::pass(format!(
                    "{} WASM artifact(s) found in target/wasm32-unknown-unknown/{profile}",
                    wasm_files.len()
                ));
            }
        }
    }

    CheckResult::warn(
        "target/wasm32-unknown-unknown exists but contains no .wasm artifacts \
         (run: cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm)"
    )
}

/// Verify required build-time dependencies (beyond the Stellar CLI, which has
/// its own dedicated check) are present on PATH: `cargo` and `rustc`.
fn check_required_dependencies() -> CheckResult {
    let mut missing = Vec::new();
    for tool in ["cargo", "rustc"] {
        let found = std::process::Command::new(tool)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !found {
            missing.push(tool);
        }
    }
    if missing.is_empty() {
        CheckResult::pass("cargo and rustc found on PATH")
    } else {
        CheckResult::fail(format!("missing required tool(s) on PATH: {}", missing.join(", ")))
    }
}

fn check_endpoint_proofs(contract_id: &str, network: &str) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let source = std::env::var("ANCHOR_ADMIN_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .map(SecretKey::new)
        .unwrap_or_else(|| SecretKey::new("default"));

    let count_str = stellar_invoke(contract_id, &source, network, &["get_attestor_count"]);
    let count: u32 = count_str.trim().trim_matches('"').parse().unwrap_or(0);
    if count == 0 {
        return results;
    }

    let list_str = stellar_invoke(contract_id, &source, network, &["list_registered_attestors"]);
    let attestors: Vec<String> = serde_json::from_str(&list_str).unwrap_or_default();

    for address in attestors {
        let proof_raw = stellar_invoke(contract_id, &source, network, &["get_endpoint_proof", "--attestor", &address]);
        
        if proof_raw.trim() == "null" || proof_raw.trim().is_empty() {
            results.push(CheckResult::warn(format!("No PoP registered for {}", address)));
            continue;
        }

        let val: serde_json::Value = match serde_json::from_str(&proof_raw) {
            Ok(v) => v,
            Err(_) => {
                results.push(CheckResult::warn(format!("No PoP registered for {}", address)));
                continue;
            }
        };

        if let Some(verified) = val.get("verified").and_then(|v| v.as_bool()) {
            if verified {
                results.push(CheckResult::pass(format!("PoP verified for {}", address)));
            } else {
                results.push(CheckResult::warn(format!("PoP registered but unverified for {}", address)));
            }
        } else {
            results.push(CheckResult::warn(format!("No PoP registered for {}", address)));
        }
    }

    results
}

fn doctor(network: &str, fix: bool) {
    println!("\n🔍 SorobanAnchor Environment Check\n");
    
    let checks = vec![
        ("Stellar CLI", check_stellar_cli()),
        ("Required Dependencies", check_required_dependencies()),
        ("WASM Target", check_wasm_target(fix)),
        ("Build Artifacts", check_build_artifacts()),
        ("Contract ID", check_contract_id_env()),
        ("Admin Secret", check_admin_secret_env()),
        ("Network", check_network_connectivity(network)),
    ];
    let mut all_passed = true;
    
    for (name, result) in &checks {
        println!("  {} {} {}", result.color(), result.icon(), name);
        println!("    {}\x1b[0m", result.message);
        if !result.passed {
            all_passed = false;
        }
    }
    
    // Optional checks that require contract ID
    if let Ok(contract_id) = std::env::var("ANCHOR_CONTRACT_ID") {
        if !contract_id.is_empty() {
            let deployment_check = check_contract_deployment(&contract_id, network);
            println!("  {} {} Contract Deployment", deployment_check.color(), deployment_check.icon());
            println!("    {}\x1b[0m", deployment_check.message);
            if !deployment_check.passed {
                all_passed = false;
            }
            
            let pop_results = check_endpoint_proofs(&contract_id, network);
            if !pop_results.is_empty() {
                println!("\n  Endpoint Proof of Possession (PoP):");
                for res in pop_results {
                    println!("    {} {} {}", res.color(), res.icon(), res.message);
                    // PoP checks are advisory and should not fail the overall doctor run
                }
            }
        }
    }
    
    let config_check = check_config_files();
    println!("  {} {} Config Files", config_check.color(), config_check.icon());
    println!("    {}\x1b[0m", config_check.message);
    if !config_check.passed {
        all_passed = false;
    }

    // ── Environment fingerprint ───────────────────────────────────────────
    println!("\n  Environment Fingerprint:");
    let fp = anchorkit::EnvironmentFingerprint::collect();
    println!("{}", fp.summary()
        .lines()
        .map(|l| format!("  {}", l))
        .collect::<std::vec::Vec<_>>()
        .join("\n"));

    println!();
    if all_passed {
        println!("✅ All checks passed! Your environment is ready.\n");
        std::process::exit(0);
    } else {
        println!("❌ Some checks failed. Please address the issues above.\n");
        if !fix {
            println!("Tip: Run with --fix to automatically resolve fixable issues.\n");
        }
        std::process::exit(1);
    }
}

// ── Health check command (#268) ───────────────────────────────────────────────

fn health_check(contract_id: &str, network: &str, source: &SecretKey, anchor: Option<&str>, attestor: Option<&str>) {
    println!("\n🏥 AnchorKit Health Check\n");

    // 1. Overall service health
    let status_raw = stellar_invoke(contract_id, source, network, &["get_health_status"]);
    let status_label = match status_raw.trim().trim_matches('"') {
        "0" | "Healthy"     => "\x1b[32m✓ Healthy\x1b[0m",
        "1" | "Degraded"    => "\x1b[33m⚠ Degraded\x1b[0m",
        _                   => "\x1b[31m✗ Unavailable\x1b[0m",
    };
    println!("  Service Status : {status_label}");

    // 2. Metadata freshness (optional — only when --anchor is supplied)
    if let Some(anchor_addr) = anchor {
        let freshness_raw = stellar_invoke(contract_id, source, network, &[
            "get_metadata_freshness",
            "--anchor", anchor_addr,
        ]);
        // Parse the returned struct fields from JSON-like output
        let state_label = if freshness_raw.contains("\"Fresh\"") || freshness_raw.contains("\"state\":0") {
            "\x1b[32mFresh\x1b[0m"
        } else if freshness_raw.contains("\"Stale\"") || freshness_raw.contains("\"state\":2") {
            "\x1b[33mStale — refresh recommended\x1b[0m"
        } else if freshness_raw.contains("\"Expired\"") || freshness_raw.contains("\"state\":3") {
            "\x1b[31mExpired — must refresh\x1b[0m"
        } else {
            "\x1b[31mMissing — no cache entry\x1b[0m"
        };
        println!("  Metadata Cache : {state_label}");
        println!("  Anchor         : {anchor_addr}");
    }

    // 3. Rate limiter health (optional — only when --attestor is supplied)
    if let Some(attestor_addr) = attestor {
        let rl_raw = stellar_invoke(contract_id, source, network, &[
            "get_rate_limiter_health",
            "--attestor", attestor_addr,
        ]);
        let throttled = rl_raw.contains("\"is_throttled\":true") || rl_raw.contains("is_throttled: true");
        let rl_label = if throttled {
            "\x1b[31m✗ Throttled\x1b[0m"
        } else {
            "\x1b[32m✓ OK\x1b[0m"
        };
        println!("  Rate Limiter   : {rl_label}");
        println!("  Attestor       : {attestor_addr}");
        if throttled {
            eprintln!("\n  ⚠  Attestor has reached the submission limit for the current window.");
        }
    }

    println!();
}

// ── Network command ───────────────────────────────────────────────────────────

fn network_cmd(action: NetworkAction) {
    match action {
        NetworkAction::Add { name, rpc_url, passphrase, horizon_url } => {
            // Validate RPC URL connectivity before saving
            let check = check_network_connectivity_url(&rpc_url);
            if !check.passed {
                eprintln!("error: RPC URL validation failed: {}", check.message);
                std::process::exit(1);
            }
            let mut profiles = load_network_profiles();
            if find_profile(&profiles, &name).is_some() {
                eprintln!("error: network '{}' already exists. Remove it first.", name);
                std::process::exit(1);
            }
            profiles.push(NetworkProfile {
                name: name.clone(),
                rpc_url,
                network_passphrase: passphrase,
                horizon_url,
                is_default: false,
            });
            save_network_profiles(&profiles);
            println!("Network '{}' added.", name);
        }
        NetworkAction::List => {
            let profiles = load_network_profiles();
            // Always show built-ins
            let builtins = [
                ("testnet",   "https://soroban-testnet.stellar.org",  "Test SDF Network ; September 2015"),
                ("mainnet",   "https://horizon.stellar.org",           "Public Global Stellar Network ; September 2015"),
                ("futurenet", "https://rpc-futurenet.stellar.org",     "Test SDF Future Network ; October 2022"),
            ];
            println!("{:<16} {:<45} {}", "NAME", "RPC URL", "PASSPHRASE");
            for (name, url, phrase) in &builtins {
                println!("{:<16} {:<45} {} (built-in)", name, url, phrase);
            }
            for p in &profiles {
                let default_marker = if p.is_default { " (default)" } else { "" };
                println!("{:<16} {:<45} {}{}", p.name, p.rpc_url, p.network_passphrase, default_marker);
            }
        }
        NetworkAction::Remove { name } => {
            let mut profiles = load_network_profiles();
            let before = profiles.len();
            profiles.retain(|p| p.name != name);
            if profiles.len() == before {
                eprintln!("error: network '{}' not found.", name);
                std::process::exit(1);
            }
            save_network_profiles(&profiles);
            println!("Network '{}' removed.", name);
        }
        NetworkAction::SetDefault { name } => {
            let mut profiles = load_network_profiles();
            // Allow setting built-in names as default (stored as a marker profile)
            let found = profiles.iter().any(|p| p.name == name);
            if !found {
                // Check if it's a built-in
                let builtins = ["testnet", "mainnet", "futurenet"];
                if !builtins.contains(&name.as_str()) {
                    eprintln!("error: network '{}' not found.", name);
                    std::process::exit(1);
                }
            }
            for p in &mut profiles {
                p.is_default = p.name == name;
            }
            save_network_profiles(&profiles);
            println!("Default network set to '{}'.", name);
        }
    }
}

fn check_network_connectivity_url(url: &str) -> CheckResult {
    match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .and_then(|client| client.get(url).send())
    {
        Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 404 => {
            CheckResult::pass(format!("RPC URL {} reachable", url))
        }
        Ok(resp) => CheckResult::warn(format!("RPC URL {} responded with HTTP {}", url, resp.status())),
        Err(e) => CheckResult::fail(format!("Cannot connect to {}: {}", url, e)),
    }
}

// ── Discover command ──────────────────────────────────────────────────────────

fn discover(anchor_url: &str, proxy_url: Option<&str>, no_proxy: Option<&str>, timeout: u64) {
    let proxy_cfg = anchorkit::ProxyConfig {
        proxy_url: proxy_url.map(|s| s.to_string()),
        no_proxy: no_proxy.map(|s| s.to_string()),
        ..anchorkit::ProxyConfig::default()
    };
    let proxy = if proxy_cfg.is_configured() { Some(&proxy_cfg) } else { None };

    match anchorkit::fetch_stellar_toml_with_proxy(anchor_url, proxy, timeout) {
        Ok(toml) => {
            let output = serde_json::json!({
                "network_passphrase": toml.network_passphrase,
                "transfer_server": toml.transfer_server,
                "transfer_server_sep0024": toml.transfer_server_sep0024,
                "kyc_server": toml.kyc_server,
                "web_auth_endpoint": toml.web_auth_endpoint,
                "signing_key": toml.signing_key,
                "direct_payment_server": toml.direct_payment_server,
                "anchor_quote_server": toml.anchor_quote_server,
                "supported_assets": toml.supported_assets,
                "capabilities": {
                    "sep6": toml.supports_sep6(),
                    "sep10": toml.supports_sep10(),
                    "sep24": toml.supports_sep24(),
                    "sep31": toml.supports_sep31(),
                    "sep38": toml.supports_sep38(),
                    "sep10_complete": toml.is_sep10_complete(),
                }
            });
            match serde_json::to_string_pretty(&output) {
                Ok(s) => println!("{s}"),
                Err(e) => { eprintln!("error: failed to serialize output: {e}"); std::process::exit(1); }
            }
        }
        Err(e) => {
            eprintln!("error: anchor discovery failed: {e}");
            std::process::exit(1);
        }
    }
}

// ── Keystore (AES-256-GCM encrypted credential store) ─────────────────────────

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use aes_gcm::aead::rand_core::RngCore;
use argon2::{Argon2, PasswordHasher, password_hash::SaltString};

fn keystore_path() -> std::path::PathBuf {
    let dir = dirs_home().join(".anchorkit");
    std::fs::create_dir_all(&dir).ok();
    dir.join("credentials.json")
}

fn keystore_load() -> std::collections::HashMap<String, String> {
    let path = keystore_path();
    if !path.exists() { return std::collections::HashMap::new(); }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&content).unwrap_or_default()
}

fn keystore_save(store: &std::collections::HashMap<String, String>) {
    let path = keystore_path();
    let json = serde_json::to_string_pretty(store).unwrap_or_default();
    std::fs::write(path, json).ok();
}

/// Derive a 32-byte key from password using Argon2id with a fixed salt derived from the name.
fn derive_key(password: &str, name: &str) -> [u8; 32] {
    let salt_raw = format!("anchorkit-{name}");
    let salt_padded = format!("{:>22}", &salt_raw[..salt_raw.len().min(22)]);
    let salt = SaltString::from_b64(&base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        salt_padded.as_bytes(),
    )).unwrap_or_else(|_| SaltString::generate(&mut rand::thread_rng()));
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)
        .unwrap_or_else(|e| { eprintln!("error: key derivation failed: {e}"); std::process::exit(1); });
    let hash_bytes = hash.hash.unwrap();
    let mut key = [0u8; 32];
    let bytes = hash_bytes.as_bytes();
    key[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);
    key
}

fn keystore_encrypt(password: &str, name: &str, plaintext: &str) -> String {
    use aes_gcm::aead::generic_array::GenericArray;
    let key_bytes = derive_key(password, name);
    let cipher = Aes256Gcm::new(GenericArray::from_slice(&key_bytes));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes())
        .unwrap_or_else(|e| { eprintln!("error: encryption failed: {e}"); std::process::exit(1); });
    // Store as base64(nonce) + "." + base64(ciphertext)
    use base64::Engine;
    format!(
        "{}.{}",
        base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
        base64::engine::general_purpose::STANDARD.encode(ciphertext),
    )
}

fn keystore_decrypt(password: &str, name: &str, stored: &str) -> Result<String, String> {
    use aes_gcm::aead::generic_array::GenericArray;
    use base64::Engine;
    let parts: Vec<&str> = stored.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err("invalid stored credential format".to_string());
    }
    let nonce_bytes = base64::engine::general_purpose::STANDARD.decode(parts[0])
        .map_err(|e| format!("base64 decode nonce: {e}"))?;
    let ciphertext = base64::engine::general_purpose::STANDARD.decode(parts[1])
        .map_err(|e| format!("base64 decode ciphertext: {e}"))?;
    let key_bytes = derive_key(password, name);
    let cipher = Aes256Gcm::new(GenericArray::from_slice(&key_bytes));
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| "decryption failed — wrong password?".to_string())?;
    String::from_utf8(plaintext).map_err(|e| format!("utf8: {e}"))
}

fn keystore_get_decrypted(name: &str, password: &str) -> SecretKey {
    let store = keystore_load();
    let stored = store.get(name)
        .unwrap_or_else(|| { eprintln!("error: credential '{}' not found", name); std::process::exit(1); });
    let plaintext = keystore_decrypt(password, name, stored)
        .unwrap_or_else(|e| { eprintln!("error: failed to decrypt credential: {e}"); std::process::exit(1); });
    SecretKey::new(plaintext)
}

fn credentials_add(name: &str, value: Option<&str>, no_interactive: bool) {
    if no_interactive {
        eprintln!("error: 'credentials add' requires interactive password prompts; \
                   not supported with --no-interactive / ANCHORKIT_NO_INTERACTIVE");
        std::process::exit(1);
    }
    let secret = match value {
        Some(v) => v.to_string(),
        None => rpassword::prompt_password("Secret key value: ")
            .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); }),
    };
    let password = rpassword::prompt_password("Keystore password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    let confirm = rpassword::prompt_password("Confirm password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    if password != confirm {
        eprintln!("error: passwords do not match");
        std::process::exit(1);
    }
    let encrypted = keystore_encrypt(&password, name, &secret);
    let mut store = keystore_load();
    store.insert(name.to_string(), encrypted);
    keystore_save(&store);
    println!("Credential '{}' stored.", name);
}

fn credentials_get(name: &str, no_interactive: bool) {
    if no_interactive {
        eprintln!("error: 'credentials get' requires an interactive password prompt; \
                   not supported with --no-interactive / ANCHORKIT_NO_INTERACTIVE");
        std::process::exit(1);
    }
    let password = rpassword::prompt_password("Keystore password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    let secret = keystore_get_decrypted(name, &password);
    println!("{}", secret.expose());
}

fn credentials_list() {
    let store = keystore_load();
    if store.is_empty() {
        println!("No credentials stored.");
    } else {
        for name in store.keys() {
            println!("{name}");
        }
    }
}

fn credentials_remove(name: &str) {
    let mut store = keystore_load();
    if store.remove(name).is_none() {
        eprintln!("error: credential '{}' not found", name);
        std::process::exit(1);
    }
    keystore_save(&store);
    println!("Credential '{}' removed.", name);
}

/// Re-encrypt every entry in `store` under `new_password`, verifying each one
/// decrypts under `old_password` first.
///
/// Decryption is attempted for every entry before any re-encryption happens,
/// so a wrong `old_password` (or a corrupted entry) leaves the keystore file
/// untouched — the caller only persists the returned map after this succeeds.
fn rotate_keystore(
    store: &std::collections::HashMap<String, String>,
    old_password: &str,
    new_password: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut decrypted: Vec<(String, String)> = Vec::with_capacity(store.len());
    for (name, stored) in store {
        let plaintext = keystore_decrypt(old_password, name, stored)
            .map_err(|e| format!("credential '{name}': {e}"))?;
        decrypted.push((name.clone(), plaintext));
    }
    let mut rotated = std::collections::HashMap::with_capacity(decrypted.len());
    for (name, plaintext) in decrypted {
        let encrypted = keystore_encrypt(new_password, &name, &plaintext);
        rotated.insert(name, encrypted);
    }
    Ok(rotated)
}

fn credentials_rotate(no_interactive: bool) {
    if no_interactive {
        eprintln!("error: 'credentials rotate' requires interactive password prompts; \
                   not supported with --no-interactive / ANCHORKIT_NO_INTERACTIVE");
        std::process::exit(1);
    }
    let store = keystore_load();
    if store.is_empty() {
        println!("No credentials stored; nothing to rotate.");
        return;
    }
    let old_password = rpassword::prompt_password("Current keystore password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    let new_password = rpassword::prompt_password("New keystore password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    let confirm = rpassword::prompt_password("Confirm new keystore password: ")
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
    if new_password != confirm {
        eprintln!("error: new passwords do not match");
        std::process::exit(1);
    }
    if new_password == old_password {
        eprintln!("error: new password must differ from the current password");
        std::process::exit(1);
    }
    let count = store.len();
    match rotate_keystore(&store, &old_password, &new_password) {
        Ok(rotated) => {
            keystore_save(&rotated);
            println!("Rotated {count} credential(s) to a new keystore password.");
        }
        Err(e) => {
            eprintln!("error: rotation aborted, no credentials were modified: {e}");
            std::process::exit(1);
        }
    }
}

// ── Offline mode (#351) ───────────────────────────────────────────────────────

/// Validate one or more config files without network access.
///
/// Returns `true` when all files pass validation, `false` otherwise.
/// Prints a pass/fail line for each file.
fn offline_validate_config(config_path: Option<&str>) -> bool {
    let paths: Vec<std::path::PathBuf> = if let Some(path) = config_path {
        vec![std::path::PathBuf::from(path)]
    } else {
        let config_dir = std::path::Path::new("configs");
        if !config_dir.exists() {
            eprintln!("  warning: configs/ directory not found");
            return true;
        }
        match std::fs::read_dir(config_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|e| e == "json" || e == "toml")
                        .unwrap_or(false)
                })
                .collect(),
            Err(e) => {
                eprintln!("  error: cannot read configs/: {e}");
                return false;
            }
        }
    };

    if paths.is_empty() {
        println!("  (no config files found)");
        return true;
    }

    let mut all_valid = true;
    for path in &paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  ✗ {}: {e}", path.display());
                all_valid = false;
                continue;
            }
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let result: Result<(), String> = match ext {
            "json" => parse_runtime_config_str(&content, ConfigFormat::Json).map(|_| ()),
            "toml" => parse_runtime_config_str(&content, ConfigFormat::Toml).map(|_| ()),
            other => Err(format!("unsupported extension: {other}")),
        };
        match result {
            Ok(_) => println!("  ✓ {}", path.display()),
            Err(e) => {
                eprintln!("  ✗ {}: {e}", path.display());
                all_valid = false;
            }
        }
    }
    all_valid
}

/// Simulate a named workflow without network access, using the given config.
fn offline_simulate(config_path: Option<&str>, workflow: &str) {
    println!("\n[offline] Simulating workflow: {workflow}");
    let config_label = config_path.unwrap_or("configs/ (default)");
    println!("[offline] Config source: {config_label}");
    println!("[offline] Validating config files...");
    let valid = offline_validate_config(config_path);
    if !valid {
        eprintln!("[offline] Config validation failed. Aborting simulation.");
        std::process::exit(1);
    }
    match workflow {
        "deploy" => {
            println!("[offline] Step 1: WASM artifact check (skipped — offline)");
            println!("[offline] Step 2: Network connectivity check (skipped — offline)");
            println!("[offline] Step 3: Simulate contract deploy (dry-run)");
            println!("[offline] ✓ Deploy simulation completed successfully.");
        }
        "register" => {
            println!("[offline] Step 1: Simulate attestor registration (dry-run)");
            println!("[offline] Step 2: Simulate service configuration (dry-run)");
            println!("[offline] ✓ Register simulation completed successfully.");
        }
        "attest" => {
            println!("[offline] Step 1: Simulate attestation submission (dry-run)");
            println!("[offline] ✓ Attest simulation completed successfully.");
        }
        other => {
            eprintln!(
                "error: unknown workflow '{other}'. Supported workflows: deploy, register, attest"
            );
            std::process::exit(1);
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────


fn verify_attestation(
    id: Option<u64>,
    payload_hash: Option<&str>,
    payload_file: Option<&str>,
    contract_id: &str,
    network: &str,
    source: &SecretKey,
) {
    // Exactly one of --id or --payload-hash must be provided.
    let (fn_args, lookup_by): (Vec<&str>, &str) = match (&id, &payload_hash) {
        (Some(i), None) => {
            let id_str = Box::leak(i.to_string().into_boxed_str());
            (vec!["get_attestation", "--id", id_str], "id")
        }
        (None, Some(h)) => (
            vec!["get_attestation_by_hash", "--issuer", "", "--payload_hash", h],
            "hash",
        ),
        (Some(_), Some(_)) => {
            eprintln!("error: --id and --payload-hash are mutually exclusive");
            std::process::exit(1);
        }
        (None, None) => {
            eprintln!("error: one of --id or --payload-hash is required");
            eprintln!("hint:  anchorkit verify --id <N>  or  anchorkit verify --payload-hash <HEX>");
            std::process::exit(1);
        }
    };

    let _ = lookup_by; // used above for clarity
    let raw = stellar_invoke(contract_id, source, network, &fn_args);
    if raw.trim().is_empty() || raw.contains("Error") || raw.contains("error") {
        eprintln!("error: Attestation not found");
        std::process::exit(1);
    }

    // Parse key fields from the returned JSON/XDR output.
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| {
        eprintln!("error: Attestation not found");
        std::process::exit(1);
    });

    let attest_id    = parsed.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let issuer       = parsed.get("issuer").and_then(|v| v.as_str()).unwrap_or("unknown");
    let subject      = parsed.get("subject").and_then(|v| v.as_str()).unwrap_or("unknown");
    let timestamp    = parsed.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
    let stored_hash  = parsed.get("payload_hash").and_then(|v| v.as_str()).unwrap_or("");

    // Optional: compare local file hash against stored hash.
    let payload_match = if let Some(path) = payload_file {
        let content = secure_read_file(path).unwrap_or_else(|e| {
            eprintln!("error: cannot read payload file '{}': {}", path, e);
            std::process::exit(1);
        });
        // Compute SHA-256 of the file content.
        let digest = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(content.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let matches = digest == stored_hash;
        Some((matches, digest))
    } else {
        None
    };

    // Print summary table.
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│ Attestation Verification Result                             │");
    println!("├─────────────────────────────────────────────────────────────┤");
    println!("│ ID           : {:<45}│", attest_id);
    println!("│ Issuer       : {:<45}│", &issuer[..issuer.len().min(45)]);
    println!("│ Subject      : {:<45}│", &subject[..subject.len().min(45)]);
    println!("│ Timestamp    : {:<45}│", timestamp);
    println!("│ Payload Hash : {:<45}│", &stored_hash[..stored_hash.len().min(45)]);
    if let Some((matches, computed)) = &payload_match {
        let label = if *matches { "✓ MATCH" } else { "✗ MISMATCH" };
        println!("│ File Hash    : {:<45}│", &computed[..computed.len().min(45)]);
        println!("│ Match        : {:<45}│", label);
    }
    println!("└─────────────────────────────────────────────────────────────┘");

    if let Some((false, _)) = payload_match {
        std::process::exit(1);
    }
}

fn main() {
    let cli = Cli::parse();
    let global_contract_id = cli.contract_id.clone();
    let no_interactive = cli.no_interactive;
    let ephemeral_token = cli.ephemeral_token.clone();
    let network = cli.network.unwrap_or_else(|| {
        let n = default_network();
        if std::env::var("STELLAR_NETWORK").is_err() && !load_network_profiles().iter().any(|p| p.is_default) {
            eprintln!("note: STELLAR_NETWORK not set — using '{n}' (set STELLAR_NETWORK or: anchorkit network set-default --name <NAME>)");
        }
        n
    });
    match cli.command {
        Commands::Deploy { network: cmd_net, source, admin, dry_run, list, upgrade, secret_key, keypair_file, yes } => {
            let net = cmd_net;
            if upgrade {
                let contract_id = require_contract_id(global_contract_id, None, "deploy --upgrade");
                let signing_source = resolve_source(
                    ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                    None, no_interactive,
                );
                upgrade_contract(&contract_id, &net, &signing_source);
            } else {
                deploy(&net, &source, admin.as_deref(), dry_run, list, yes, no_interactive);
            }
        }
        Commands::Register { address, services, contract_id, network: cmd_net, secret_key, keypair_file, credential_name, sep10_token, sep10_issuer } => {
            let cid = require_contract_id(global_contract_id, contract_id, "register");
            let net = cmd_net;
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                credential_name.as_deref(), no_interactive,
            );
            register(&address, &services, &cid, &net, &source, &sep10_token, &sep10_issuer);
        }
        Commands::Attest { subject, payload_hash, contract_id, network: cmd_net, secret_key, keypair_file, credential_name, issuer, session_id, signing_key } => {
            let cid = require_contract_id(global_contract_id, contract_id, "attest");
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                credential_name.as_deref(), no_interactive,
            );
            attest(&subject, &payload_hash, &cid, &cmd_net, &source, &issuer, session_id, signing_key.as_deref());
        }
        Commands::Quote { from, to, amount, contract_id, network: cmd_net, secret_key, keypair_file, credential_name } => {
            let cid = require_contract_id(global_contract_id, contract_id, "quote");
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                credential_name.as_deref(), no_interactive,
            );
            quote(&from, &to, amount, &cid, &cmd_net, &source);
        }
        Commands::Status { tx_id, anchor_url, proxy_url, no_proxy } => {
            status(&tx_id, &anchor_url, proxy_url.as_deref(), no_proxy.as_deref());
        }
        Commands::Revoke { address, contract_id, network: cmd_net, secret_key, keypair_file, credential_name } => {
            let cid = require_contract_id(global_contract_id, contract_id, "revoke");
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                credential_name.as_deref(), no_interactive,
            );
            revoke(&address, &cid, &cmd_net, &source);
        }
        Commands::Doctor { fix } => {
            doctor(&network, fix);
        }
        Commands::Health { contract_id, network: cmd_net, secret_key, keypair_file, anchor, attestor } => {
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                None, no_interactive,
            );
            health_check(&contract_id, &cmd_net, &source, anchor.as_deref(), attestor.as_deref());
        }
        Commands::Network { action } => {
            network_cmd(action);
        }
        Commands::Discover { anchor_url, proxy_url, no_proxy, timeout } => {
            discover(&anchor_url, proxy_url.as_deref(), no_proxy.as_deref(), timeout);
        }
        Commands::Credentials { action } => {
            match action {
                CredentialsAction::Add { name, value } => {
                    credentials_add(&name, value.as_deref(), no_interactive);
                }
                CredentialsAction::Get { name } => {
                    credentials_get(&name, no_interactive);
                }
                CredentialsAction::List => {
                    credentials_list();
                }
                CredentialsAction::Remove { name } => {
                    credentials_remove(&name);
                }
                CredentialsAction::Rotate => {
                    credentials_rotate(no_interactive);
                }
            }
        }
        Commands::Offline { action } => match action {
            OfflineAction::Validate { config } => {
                println!("\n[offline] Config validation\n");
                let ok = offline_validate_config(config.as_deref());
                if ok {
                    println!("\n✅ All config files are valid.\n");
                } else {
                    eprintln!("\n❌ One or more config files failed validation.\n");
                    std::process::exit(1);
                }
            }
            OfflineAction::Simulate { config, workflow } => {
                offline_simulate(config.as_deref(), &workflow);
            }
        },
        Commands::Verify { id, payload_hash, payload_file, contract_id, network: cmd_net, secret_key, keypair_file } => {
            let cid = require_contract_id(global_contract_id, contract_id, "verify");
            let source = resolve_source(
                ephemeral_token.as_deref(), secret_key.as_deref(), keypair_file.as_deref(),
                None, no_interactive,
            );
            verify_attestation(id, payload_hash.as_deref(), payload_file.as_deref(), &cid, &cmd_net, &source);
        }
    }
}

#[cfg(test)]
mod secret_resolution_tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> { None }
    fn env_with<'a>(key: &'a str, value: &'a str) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| if k == key { Some(value.to_string()) } else { None }
    }

    const VALID_KEY: &str = "SCZANGBA5IIPMEFXBI5LZU7RVJZOLBYHJYFJ2KYN3CQPUOVFRDPCNTY";

    #[test]
    fn test_validate_stellar_secret_accepts_valid_key() {
        assert!(validate_stellar_secret(VALID_KEY, "test").is_ok());
    }

    #[test]
    fn test_validate_stellar_secret_rejects_empty() {
        let err = validate_stellar_secret("", "test").unwrap_err();
        assert!(err.contains("must not be empty"), "got: {err}");
    }

    #[test]
    fn test_validate_stellar_secret_rejects_non_s_prefix() {
        let err = validate_stellar_secret("GABCDE123", "test").unwrap_err();
        assert!(err.contains("'S...' format"), "got: {err}");
    }

    #[test]
    fn test_resolve_uses_explicit_secret_key_first() {
        let result = try_resolve_source(
            None, Some(VALID_KEY), None, None, false,
            &env_with("ANCHOR_ADMIN_SECRET", VALID_KEY),
        );
        assert_eq!(result.unwrap(), VALID_KEY);
    }

    #[test]
    fn test_resolve_uses_ephemeral_token_over_secret_key() {
        let result = try_resolve_source(
            Some(VALID_KEY), Some("Sother"), None, None, false, &no_env,
        );
        assert_eq!(result.unwrap(), VALID_KEY);
    }

    #[test]
    fn test_resolve_falls_back_to_env_var() {
        let result = try_resolve_source(None, None, None, None, false, &env_with("ANCHOR_ADMIN_SECRET", VALID_KEY));
        assert_eq!(result.unwrap(), VALID_KEY);
    }

    #[test]
    fn test_resolve_errors_on_empty_env_var() {
        let err = try_resolve_source(
            None, None, None, None, false,
            &env_with("ANCHOR_ADMIN_SECRET", ""),
        )
        .unwrap_err();
        assert!(err.contains("empty"), "expected 'empty' in: {err}");
    }

    #[test]
    fn test_resolve_errors_on_invalid_env_var_format() {
        let err = try_resolve_source(
            None, None, None, None, false,
            &env_with("ANCHOR_ADMIN_SECRET", "GABCDE123"),
        )
        .unwrap_err();
        assert!(err.contains("'S...' format"), "expected format error in: {err}");
    }

    #[test]
    fn test_resolve_errors_when_no_source_provided() {
        let err = try_resolve_source(None, None, None, None, false, &no_env).unwrap_err();
        assert!(err.contains("signing key required"), "got: {err}");
    }

    #[test]
    fn test_resolve_errors_on_credential_name_in_non_interactive_mode() {
        let err = try_resolve_source(
            None, None, None, Some("my-cred"), true, &no_env,
        )
        .unwrap_err();
        assert!(err.contains("non-interactive"), "got: {err}");
    }

    #[test]
    fn test_secret_key_redacted_in_display() {
        let sk = SecretKey::new(VALID_KEY);
        assert_eq!(format!("{sk}"), "[REDACTED]");
        assert_eq!(format!("{sk:?}"), "[REDACTED]");
    }

    #[test]
    fn test_secret_key_deref_exposes_value() {
        let sk = SecretKey::new("STEST");
        let s: &str = &sk;
        assert_eq!(s, "STEST");
    }

    #[test]
    fn test_secret_key_expose_method() {
        let sk = SecretKey::new("STEST");
        assert_eq!(sk.expose(), "STEST");
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    #[test]
    fn test_offline_validate_nonexistent_path() {
        let result = offline_validate_config(Some("/nonexistent/path/config.json"));
        assert!(!result, "nonexistent file should fail validation");
    }

    #[test]
    fn test_offline_validate_valid_json_written_to_tempdir() {
        let dir = std::env::temp_dir();
        let path = dir.join("anchorkit_test_valid.json");
        // Must be schema-valid, not just syntactically valid JSON: offline_validate_config
        // now runs full RuntimeConfig schema validation (see #634), not just a parse check.
        std::fs::write(&path, r#"{
            "contract": {"name": "test-anchor", "version": "1.0.0", "network": "stellar-testnet"},
            "attestors": {"registry": [
                {"name": "test-attestor", "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ", "role": "kyc-issuer", "enabled": true}
            ]}
        }"#).unwrap();
        let result = offline_validate_config(Some(path.to_str().unwrap()));
        let _ = std::fs::remove_file(&path);
        assert!(result, "schema-valid JSON should pass validation");
    }

    #[test]
    fn test_offline_validate_syntactically_valid_but_schema_invalid_json() {
        let dir = std::env::temp_dir();
        let path = dir.join("anchorkit_test_schema_invalid.json");
        // Well-formed JSON that does not match the RuntimeConfig schema (missing
        // required fields, unknown top-level shape) must now be rejected.
        std::fs::write(&path, r#"{"network":"testnet","admin":"GABC"}"#).unwrap();
        let result = offline_validate_config(Some(path.to_str().unwrap()));
        let _ = std::fs::remove_file(&path);
        assert!(!result, "schema-invalid JSON must fail validation even though it parses");
    }

    #[test]
    fn test_offline_validate_invalid_json_written_to_tempdir() {
        let dir = std::env::temp_dir();
        let path = dir.join("anchorkit_test_invalid.json");
        std::fs::write(&path, r#"{not valid json"#).unwrap();
        let result = offline_validate_config(Some(path.to_str().unwrap()));
        let _ = std::fs::remove_file(&path);
        assert!(!result, "invalid JSON should fail validation");
    }

}

#[cfg(test)]
mod keystore_tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let stored = keystore_encrypt("hunter2", "alice", "SECRET-VALUE");
        let plaintext = keystore_decrypt("hunter2", "alice", &stored).unwrap();
        assert_eq!(plaintext, "SECRET-VALUE");
    }

    #[test]
    fn test_decrypt_with_wrong_password_fails_clearly() {
        let stored = keystore_encrypt("hunter2", "alice", "SECRET-VALUE");
        let err = keystore_decrypt("wrong-password", "alice", &stored).unwrap_err();
        assert!(err.contains("wrong password"), "got: {err}");
    }

    #[test]
    fn test_decrypt_with_wrong_name_fails() {
        // The name is part of the salt, so decrypting under a different name
        // (different derived key) must fail even with the right password.
        let stored = keystore_encrypt("hunter2", "alice", "SECRET-VALUE");
        assert!(keystore_decrypt("hunter2", "bob", &stored).is_err());
    }

    #[test]
    fn test_encrypt_is_nondeterministic_due_to_random_nonce() {
        let a = keystore_encrypt("hunter2", "alice", "SECRET-VALUE");
        let b = keystore_encrypt("hunter2", "alice", "SECRET-VALUE");
        assert_ne!(a, b, "ciphertext should differ across calls due to a fresh random nonce");
    }

    #[test]
    fn test_rotate_keystore_reencrypts_all_entries_under_new_password() {
        let mut store = std::collections::HashMap::new();
        store.insert("alice".to_string(), keystore_encrypt("old-pw", "alice", "SECRET-A"));
        store.insert("bob".to_string(), keystore_encrypt("old-pw", "bob", "SECRET-B"));

        let rotated = rotate_keystore(&store, "old-pw", "new-pw").unwrap();

        assert_eq!(rotated.len(), 2);
        assert_eq!(keystore_decrypt("new-pw", "alice", &rotated["alice"]).unwrap(), "SECRET-A");
        assert_eq!(keystore_decrypt("new-pw", "bob", &rotated["bob"]).unwrap(), "SECRET-B");
        // Old password must no longer work post-rotation.
        assert!(keystore_decrypt("old-pw", "alice", &rotated["alice"]).is_err());
    }

    #[test]
    fn test_rotate_keystore_rejects_wrong_current_password_without_modifying_data() {
        let mut store = std::collections::HashMap::new();
        store.insert("alice".to_string(), keystore_encrypt("old-pw", "alice", "SECRET-A"));

        let err = rotate_keystore(&store, "definitely-wrong", "new-pw").unwrap_err();
        assert!(err.contains("alice"), "error should identify the failing credential: {err}");

        // The original store passed in must be untouched by the failed attempt.
        assert_eq!(
            keystore_decrypt("old-pw", "alice", &store["alice"]).unwrap(),
            "SECRET-A"
        );
    }

    #[test]
    fn test_rotate_keystore_empty_store_succeeds_trivially() {
        let store = std::collections::HashMap::new();
        let rotated = rotate_keystore(&store, "old-pw", "new-pw").unwrap();
        assert!(rotated.is_empty());
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    const VALID_RUNTIME_CONFIG_JSON: &str = r#"{
        "contract": {"name": "test-anchor", "version": "1.0.0", "network": "stellar-testnet"},
        "attestors": {"registry": [
            {"name": "test-attestor", "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ", "role": "kyc-issuer", "enabled": true}
        ]}
    }"#;

    fn tempdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("anchorkit_doctor_test_{label}_{:?}", std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_check_config_files_missing_dir_warns() {
        let dir = std::env::temp_dir().join("anchorkit_doctor_definitely_missing_dir");
        let _ = std::fs::remove_dir_all(&dir);
        let result = check_config_files_in(&dir);
        assert!(result.passed, "missing configs/ dir should warn, not fail");
        assert!(result.warning);
    }

    #[test]
    fn test_check_config_files_empty_dir_warns() {
        let dir = tempdir("empty");
        let result = check_config_files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.passed);
        assert!(result.warning, "empty configs/ dir should warn (no files found)");
    }

    #[test]
    fn test_check_config_files_valid_schema_passes() {
        let dir = tempdir("valid");
        std::fs::write(dir.join("anchor.json"), VALID_RUNTIME_CONFIG_JSON).unwrap();
        let result = check_config_files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.passed && !result.warning, "got: {}", result.message);
    }

    #[test]
    fn test_check_config_files_schema_invalid_fails() {
        let dir = tempdir("invalid");
        // Syntactically valid JSON, but missing required 'contract'/'attestors' fields.
        std::fs::write(dir.join("anchor.json"), r#"{"network":"testnet"}"#).unwrap();
        let result = check_config_files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!result.passed, "schema-invalid config must fail the doctor check");
        assert!(result.message.contains("anchor.json"), "got: {}", result.message);
    }

    #[test]
    fn test_check_config_files_malformed_toml_fails() {
        // Regression test: the previous implementation always counted TOML
        // files as valid without actually parsing them.
        let dir = tempdir("badtoml");
        std::fs::write(dir.join("anchor.toml"), "this is not [valid toml").unwrap();
        let result = check_config_files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!result.passed, "malformed TOML must now be caught, got: {}", result.message);
    }

    #[test]
    fn test_check_build_artifacts_missing_target_dir_warns() {
        let dir = tempdir("no_target");
        let result = check_build_artifacts_at(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.passed);
        assert!(result.warning, "missing target dir should warn, not fail");
    }

    #[test]
    fn test_check_build_artifacts_empty_target_dir_warns() {
        let dir = tempdir("empty_target");
        std::fs::create_dir_all(dir.join("target/wasm32-unknown-unknown/release")).unwrap();
        let result = check_build_artifacts_at(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.passed);
        assert!(result.warning, "target dir with no .wasm files should warn");
    }

    #[test]
    fn test_check_build_artifacts_found_passes() {
        let dir = tempdir("with_wasm");
        let release_dir = dir.join("target/wasm32-unknown-unknown/release");
        std::fs::create_dir_all(&release_dir).unwrap();
        std::fs::write(release_dir.join("anchorkit.wasm"), b"\0asm").unwrap();
        let result = check_build_artifacts_at(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.passed && !result.warning, "got: {}", result.message);
    }

    #[test]
    fn test_check_required_dependencies_passes_in_dev_environment() {
        // This test suite only runs under `cargo test`, so cargo (and the
        // rustc it invokes) are necessarily on PATH.
        let result = check_required_dependencies();
        assert!(result.passed && !result.warning, "got: {}", result.message);
    }
}

#[cfg(test)]
mod cli_validation_tests {
    use super::*;

    const SAMPLE_SECRET_KEY: &str = "SCZANGBA5IIPMEFXBI5LZU7RVJZOLBYHJYFJ2KYN3CQPUOVFRDPCNTY";

    // ── validate_admin_arg ──────────────────────────────────────────────

    #[test]
    fn test_validate_admin_arg_accepts_none() {
        assert!(validate_admin_arg(None).is_ok());
    }

    #[test]
    fn test_validate_admin_arg_accepts_default_alias() {
        assert!(validate_admin_arg(Some("default")).is_ok());
    }

    #[test]
    fn test_validate_admin_arg_accepts_valid_public_address() {
        // A syntactically valid Stellar public address (G... + valid checksum).
        let valid = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let result = validate_admin_arg(Some(valid));
        assert!(result.is_ok(), "expected valid address to pass, got: {result:?}");
    }

    #[test]
    fn test_validate_admin_arg_rejects_malformed_address() {
        let err = validate_admin_arg(Some("not-an-address")).unwrap_err();
        assert!(err.contains("invalid --admin address"), "got: {err}");
    }

    #[test]
    fn test_validate_admin_arg_rejects_secret_key_instead_of_public() {
        // A secret key (S...) is not a valid public admin address.
        let err = validate_admin_arg(Some(SAMPLE_SECRET_KEY)).unwrap_err();
        assert!(err.contains("invalid --admin address"), "got: {err}");
    }

    // ── validate_services_arg ───────────────────────────────────────────

    #[test]
    fn test_validate_services_arg_rejects_empty() {
        let err = validate_services_arg(&[]).unwrap_err();
        assert!(err.contains("at least one service"), "got: {err}");
    }

    #[test]
    fn test_validate_services_arg_accepts_non_empty() {
        let services = vec!["deposits".to_string()];
        assert!(validate_services_arg(&services).is_ok());
    }

    // ── confirm_mainnet_deploy ───────────────────────────────────────────

    #[test]
    fn test_confirm_mainnet_deploy_skips_prompt_for_non_mainnet() {
        assert!(confirm_mainnet_deploy("testnet", false, false));
    }

    #[test]
    fn test_confirm_mainnet_deploy_skips_prompt_with_yes_flag() {
        assert!(confirm_mainnet_deploy("mainnet", true, false));
    }

    #[test]
    fn test_confirm_mainnet_deploy_skips_prompt_with_no_interactive() {
        assert!(confirm_mainnet_deploy("mainnet", false, true));
    }
}
