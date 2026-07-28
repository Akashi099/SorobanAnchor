//! Runtime configuration loading and shape validation.

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};

use serde::Deserialize;
use serde_json::Value;

#[cfg(feature = "std")]
use std::{fs, path::Path};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub contract: ContractConfig,
    pub attestors: AttestorsConfig,
    pub sessions: Option<SessionsConfig>,
    pub operations: Option<OperationsConfig>,
    pub remittance: Option<RemittanceConfig>,
    pub stablecoin: Option<StablecoinConfig>,
    pub compliance: Option<Value>,
    pub storage: Option<StorageConfig>,
    pub security: Option<SecurityConfig>,
    pub monitoring: Option<MonitoringConfig>,
    /// Optional proxy configuration for HTTP-based anchor discovery and webhook delivery.
    pub proxy: Option<ProxyConfig>,
}

/// Proxy settings embedded in the runtime configuration file.
///
/// When present, all outbound HTTP requests (stellar.toml discovery, webhook
/// delivery, SEP-6 status checks) route through the selected proxy.
/// Scheme-specific proxies (`http_proxy_url` / `https_proxy_url`) take
/// precedence over the catch-all `proxy_url`; hosts on `no_proxy` bypass all
/// proxies. Optional `credentials` authenticate to the proxy via HTTP Basic.
///
/// The configuration is validated on load: malformed proxy URLs, credentials
/// without a username, and credentials supplied without any proxy URL are all
/// rejected (see [`ProxyConfig::validate`]).
///
/// # Example (JSON)
///
/// ```json
/// {
///   "proxy": {
///     "proxy_url": "http://proxy.corp.example.com:3128",
///     "https_proxy_url": "http://tls-proxy.corp.example.com:3129",
///     "no_proxy": "localhost,127.0.0.1",
///     "credentials": { "username": "svc-anchor", "password": "s3cret" }
///   }
/// }
/// ```
///
/// These are re-exports of [`http_client::ProxyConfig`] /
/// [`http_client::ProxyCredentials`] so that config files and the HTTP client
/// share a single type.
pub use crate::http_client::{ProxyConfig, ProxyCredentials};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContractConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub network: String,
    pub admin_address: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AttestorsConfig {
    pub registry: Vec<AttestorConfig>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AttestorConfig {
    pub name: String,
    pub address: String,
    pub description: Option<String>,
    pub endpoint: Option<String>,
    pub contact_email: Option<String>,
    pub role: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionsConfig {
    pub enable_session_tracking: Option<bool>,
    pub session_timeout_seconds: Option<u64>,
    pub operations_per_session: Option<u64>,
    pub audit_log_retention_days: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationsConfig {
    pub templates: Option<Vec<OperationTemplateConfig>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationTemplateConfig {
    pub id: String,
    pub name: String,
    pub attestor: String,
    pub operation_type: String,
    pub required_fields: Vec<String>,
    pub replay_protection: String,
    pub description: Option<String>,
    pub payload_schema: Option<Value>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemittanceConfig {
    pub corridors: Option<Vec<RemittanceCorridorConfig>>,
    pub exchange_rate: Option<ExchangeRateConfig>,
    pub fee_structure: Option<Vec<FeeStructureConfig>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemittanceCorridorConfig {
    pub source: String,
    pub destination: String,
    pub local_currency: String,
    pub settlement_method: String,
    pub expected_settlement_hours: Option<u64>,
    pub minimum_amount: Option<f64>,
    pub maximum_amount: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExchangeRateConfig {
    pub enable_live_rates: Option<bool>,
    pub rate_lock_duration_seconds: Option<u64>,
    pub rate_variance_tolerance_percent: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FeeStructureConfig {
    pub corridor: String,
    pub fee_type: String,
    pub fee_value: f64,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StablecoinConfig {
    pub name: String,
    pub symbol: String,
    pub decimals: u64,
    pub reserve_currency: String,
    pub reserve_composition: Option<Vec<ReserveCompositionConfig>>,
    pub supply_caps: Option<SupplyCapsConfig>,
    pub collateral_types: Option<Vec<CollateralTypeConfig>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReserveCompositionConfig {
    pub asset: String,
    pub target_percentage: f64,
    pub minimum_percentage: f64,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SupplyCapsConfig {
    pub maximum_supply_cap: Option<u64>,
    pub warning_threshold_percent: Option<f64>,
    pub emergency_threshold_percent: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CollateralTypeConfig {
    pub name: String,
    pub symbol: String,
    pub liquidation_ratio: f64,
    pub liquidation_fee_percent: Option<f64>,
    pub price_feed: Option<String>,
    pub minimum_deposit: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub instance_ttl_days: Option<u64>,
    pub session_cache_enabled: Option<bool>,
    pub persistent_ttl_days: Option<u64>,
    pub audit_log_enabled: Option<bool>,
    pub audit_log_compression: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    pub require_signature_verification: Option<bool>,
    pub signature_algorithm: Option<String>,
    pub signature_expiry_seconds: Option<u64>,
    pub nonce_required: Option<bool>,
    pub nonce_reuse_prevention: Option<bool>,
    pub endpoint_pins: Option<Vec<EndpointPinConfig>>,
    pub rate_limits: Option<Vec<RateLimitConfig>>,
    pub multisig_requirements: Option<Vec<MultisigRequirementConfig>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EndpointPinConfig {
    pub endpoint: String,
    pub pin_sha256: String,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    pub attestor: String,
    pub requests_per_minute: u64,
    pub requests_per_hour: u64,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MultisigRequirementConfig {
    pub operation: String,
    pub required_signatures: u64,
    pub signatory_attestors: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MonitoringConfig {
    pub enable_metrics: Option<bool>,
    pub log_all_operations: Option<bool>,
    pub alert_on_failed_attestations: Option<bool>,
    pub alert_on_replay_attempts: Option<bool>,
    pub metrics_namespace: Option<String>,
    pub alerts: Option<Vec<AlertConfig>>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct AlertConfig {
    pub condition: String,
    pub severity: String,
    pub recipients: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub fn parse_runtime_config_str(input: &str, format: ConfigFormat) -> Result<RuntimeConfig, String> {
    let config = match format {
        ConfigFormat::Json => serde_json::from_str(input).map_err(|err| err.to_string())?,
        ConfigFormat::Toml => toml::from_str(input).map_err(|err| err.to_string())?,
    };
    validate_runtime_config(&config)?;
    Ok(config)
}

#[cfg(feature = "std")]
pub fn load_runtime_config_file(path: impl AsRef<Path>) -> Result<RuntimeConfig, String> {
    let path = path.as_ref();
    let input = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let format = ConfigFormat::from_path(path)?;
    parse_runtime_config_str(&input, format)
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ConfigFormat {
    Json,
    Toml,
}

impl ConfigFormat {
    #[cfg(feature = "std")]
    fn from_path(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => Ok(Self::Json),
            Some("toml") => Ok(Self::Toml),
            Some(ext) => Err(format!("unsupported config extension: {ext}")),
            None => Err("config path has no extension".to_string()),
        }
    }
}

fn validate_runtime_config(config: &RuntimeConfig) -> Result<(), String> {
    if config.contract.name.is_empty() {
        return Err("contract.name cannot be empty".to_string());
    }

    if config.attestors.registry.is_empty() {
        return Err("attestors.registry cannot be empty".to_string());
    }

    let attestors: Vec<&str> = config
        .attestors
        .registry
        .iter()
        .map(|attestor| attestor.name.as_str())
        .collect();

    if let Some(operations) = &config.operations {
        if let Some(templates) = &operations.templates {
            for template in templates {
                if !attestors.contains(&template.attestor.as_str()) {
                    return Err(format!(
                        "operation '{}' references unknown attestor '{}'",
                        template.id, template.attestor
                    ));
                }
            }
        }
    }

    if let Some(security) = &config.security {
        if let Some(rate_limits) = &security.rate_limits {
            for rate_limit in rate_limits {
                if !attestors.contains(&rate_limit.attestor.as_str()) {
                    return Err(format!(
                        "rate limit references unknown attestor '{}'",
                        rate_limit.attestor
                    ));
                }
            }
        }

        if let Some(requirements) = &security.multisig_requirements {
            for requirement in requirements {
                for signatory in &requirement.signatory_attestors {
                    if !attestors.contains(&signatory.as_str()) {
                        return Err(format!(
                            "multisig requirement '{}' references unknown attestor '{}'",
                            requirement.operation, signatory
                        ));
                    }
                }
            }
        }
    }

    if let Some(proxy) = &config.proxy {
        proxy.validate().map_err(|err| format!("proxy: {err}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod proxy_config_tests {
    use super::*;

    /// Minimal valid config JSON used as a base for proxy tests.
    fn base_config_json(proxy_section: &str) -> String {
        alloc::format!(
            r#"{{
                "contract": {{
                    "name": "TestAnchor",
                    "version": "1.0.0",
                    "network": "testnet"
                }},
                "attestors": {{
                    "registry": [{{
                        "name": "attestor-1",
                        "address": "GABC123",
                        "role": "primary",
                        "enabled": true
                    }}]
                }}
                {proxy_section}
            }}"#
        )
    }

    #[test]
    fn test_config_without_proxy_parses_successfully() {
        let json = base_config_json("");
        let config = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap();
        assert!(config.proxy.is_none(), "proxy should be None when absent");
    }

    #[test]
    fn test_config_with_proxy_url_parses_correctly() {
        let proxy_section = r#","proxy": {"proxy_url": "http://proxy.corp.example.com:3128", "no_proxy": null}"#;
        let json = base_config_json(proxy_section);
        let config = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap();
        let proxy = config.proxy.expect("proxy should be Some");
        assert_eq!(
            proxy.proxy_url.as_deref(),
            Some("http://proxy.corp.example.com:3128")
        );
        assert!(proxy.no_proxy.is_none());
        assert!(proxy.is_configured());
    }

    #[test]
    fn test_config_with_proxy_url_and_no_proxy_list() {
        let proxy_section = r#","proxy": {"proxy_url": "http://proxy.corp.example.com:3128", "no_proxy": "localhost,127.0.0.1"}"#;
        let json = base_config_json(proxy_section);
        let config = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap();
        let proxy = config.proxy.expect("proxy should be Some");
        assert_eq!(
            proxy.proxy_url.as_deref(),
            Some("http://proxy.corp.example.com:3128")
        );
        assert_eq!(proxy.no_proxy.as_deref(), Some("localhost,127.0.0.1"));
    }

    #[test]
    fn test_config_with_null_proxy_fields() {
        let proxy_section = r#","proxy": {"proxy_url": null, "no_proxy": null}"#;
        let json = base_config_json(proxy_section);
        let config = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap();
        let proxy = config.proxy.expect("proxy key present → Some");
        assert!(proxy.proxy_url.is_none());
        assert!(proxy.no_proxy.is_none());
        assert!(!proxy.is_configured(), "null proxy_url means not configured");
    }

    #[test]
    fn test_config_proxy_is_configured_helper() {
        let configured = ProxyConfig {
            proxy_url: Some("http://proxy.example.com:3128".to_string()),
            no_proxy: None,
            ..ProxyConfig::default()
        };
        assert!(configured.is_configured());

        let unconfigured = ProxyConfig::default();
        assert!(!unconfigured.is_configured());
    }

    #[test]
    fn test_config_toml_with_proxy() {
        let toml_input = r#"
[contract]
name = "TestAnchor"
version = "1.0.0"
network = "testnet"

[[attestors.registry]]
name = "attestor-1"
address = "GABC123"
role = "primary"
enabled = true

[proxy]
proxy_url = "http://proxy.corp.example.com:3128"
no_proxy = "localhost"
"#;
        let config = parse_runtime_config_str(toml_input, ConfigFormat::Toml).unwrap();
        let proxy = config.proxy.expect("proxy should be Some");
        assert_eq!(
            proxy.proxy_url.as_deref(),
            Some("http://proxy.corp.example.com:3128")
        );
        assert_eq!(proxy.no_proxy.as_deref(), Some("localhost"));
    }

    // ── Proxy credentials and validation (#606) ───────────────────────────────

    #[test]
    fn test_config_with_proxy_credentials_and_per_scheme_urls() {
        let proxy_section = r#","proxy": {
            "proxy_url": "http://proxy.corp.example.com:3128",
            "https_proxy_url": "http://tls-proxy.corp.example.com:3129",
            "no_proxy": "localhost",
            "credentials": {"username": "svc-anchor", "password": "s3cret"}
        }"#;
        let json = base_config_json(proxy_section);
        let config = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap();
        let proxy = config.proxy.expect("proxy should be Some");
        assert_eq!(
            proxy.https_proxy_url.as_deref(),
            Some("http://tls-proxy.corp.example.com:3129")
        );
        let creds = proxy.credentials.as_ref().expect("credentials should be Some");
        assert_eq!(creds.username, "svc-anchor");
        assert_eq!(creds.password, "s3cret");
        assert!(proxy.has_credentials());
    }

    #[test]
    fn test_config_toml_with_proxy_credentials() {
        let toml_input = r#"
[contract]
name = "TestAnchor"
version = "1.0.0"
network = "testnet"

[[attestors.registry]]
name = "attestor-1"
address = "GABC123"
role = "primary"
enabled = true

[proxy]
http_proxy_url = "http://http-proxy.corp:3128"
https_proxy_url = "http://tls-proxy.corp:3129"

[proxy.credentials]
username = "svc-anchor"
password = "s3cret"
"#;
        let config = parse_runtime_config_str(toml_input, ConfigFormat::Toml).unwrap();
        let proxy = config.proxy.expect("proxy should be Some");
        assert_eq!(proxy.http_proxy_url.as_deref(), Some("http://http-proxy.corp:3128"));
        assert_eq!(
            proxy.credentials.as_ref().map(|c| c.username.as_str()),
            Some("svc-anchor")
        );
    }

    #[test]
    fn test_config_rejects_invalid_proxy_scheme_at_parse_time() {
        let proxy_section = r#","proxy": {"proxy_url": "socks5://proxy.corp:1080"}"#;
        let json = base_config_json(proxy_section);
        let err = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap_err();
        assert!(err.contains("invalid proxy URL"), "got: {err}");
        assert!(err.starts_with("proxy:"), "error should be scoped to proxy, got: {err}");
    }

    #[test]
    fn test_config_rejects_credentials_without_proxy_url() {
        let proxy_section =
            r#","proxy": {"credentials": {"username": "svc", "password": "pw"}}"#;
        let json = base_config_json(proxy_section);
        let err = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap_err();
        assert!(err.contains("no proxy URL configured"), "got: {err}");
    }

    #[test]
    fn test_config_rejects_credentials_missing_password() {
        let proxy_section = r#","proxy": {"proxy_url": "http://proxy.corp:3128", "credentials": {"username": "svc"}}"#;
        let json = base_config_json(proxy_section);
        assert!(
            parse_runtime_config_str(&json, ConfigFormat::Json).is_err(),
            "credentials without a password field must be rejected"
        );
    }

    #[test]
    fn test_config_rejects_unknown_proxy_field() {
        let proxy_section = r#","proxy": {"proxy_url": "http://proxy.corp:3128", "pasword": "typo"}"#;
        let json = base_config_json(proxy_section);
        assert!(
            parse_runtime_config_str(&json, ConfigFormat::Json).is_err(),
            "unknown proxy fields (likely typos) must be rejected"
        );
    }

    #[test]
    fn test_config_rejects_empty_credential_username() {
        let proxy_section = r#","proxy": {"proxy_url": "http://proxy.corp:3128", "credentials": {"username": "", "password": "pw"}}"#;
        let json = base_config_json(proxy_section);
        let err = parse_runtime_config_str(&json, ConfigFormat::Json).unwrap_err();
        assert!(err.contains("username cannot be empty"), "got: {err}");
    }
}
