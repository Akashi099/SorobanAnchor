//! Integration tests for issue #606 — proxy routing and credential handling
//! for outbound requests.
//!
//! Covers the acceptance criteria end to end:
//! - configured proxies are selected per scheme with `no_proxy` bypass;
//! - request credentials are injected into constructed requests;
//! - invalid configurations are rejected;
//! - secrets never appear in `Debug` output.
//!
//! Everything here runs without the `std` feature (transports are injected),
//! so the suite is exercised by `cargo test --no-default-features`.

#![cfg(not(feature = "wasm"))]

use anchorkit::http_client::{
    post_with_options, verify_outbound_signature, OutboundRequestOptions, ProxyConfig,
    ProxyCredentials, RequestCredentials,
};

fn corp_proxy_config() -> ProxyConfig {
    ProxyConfig {
        proxy_url: Some("http://proxy.corp:3128".to_string()),
        http_proxy_url: Some("http://http-proxy.corp:3180".to_string()),
        https_proxy_url: Some("http://tls-proxy.corp:3129".to_string()),
        no_proxy: Some("localhost,127.0.0.1,.internal.corp".to_string()),
        credentials: Some(ProxyCredentials {
            username: "svc-anchor".to_string(),
            password: "proxy-pw".to_string(),
        }),
    }
}

// ── Proxy selection ──────────────────────────────────────────────────────────

#[test]
fn proxy_selection_routing_matrix() {
    let cfg = corp_proxy_config();
    let cases: &[(&str, Option<&str>)] = &[
        // Scheme-specific proxies win over the catch-all.
        ("https://anchor.example.com/sep6", Some("http://tls-proxy.corp:3129")),
        ("http://anchor.example.com/sep6", Some("http://http-proxy.corp:3180")),
        // no_proxy bypass: exact host, loopback with port, subdomain suffix.
        ("https://localhost/health", None),
        ("http://127.0.0.1:8080/metrics", None),
        ("https://api.internal.corp/v1", None),
        ("https://internal.corp/v1", None),
        // Lookalike hosts are still proxied.
        ("https://fake-internal.corp.example.com", Some("http://tls-proxy.corp:3129")),
    ];
    for (url, expected) in cases {
        assert_eq!(
            cfg.select_proxy_url(url),
            *expected,
            "unexpected proxy selection for {url}"
        );
    }
}

#[test]
fn catch_all_proxy_applies_to_both_schemes() {
    let cfg = ProxyConfig {
        proxy_url: Some("http://proxy.corp:3128".to_string()),
        ..ProxyConfig::default()
    };
    assert_eq!(cfg.select_proxy_url("https://a.example.com"), Some("http://proxy.corp:3128"));
    assert_eq!(cfg.select_proxy_url("http://a.example.com"), Some("http://proxy.corp:3128"));
    assert!(cfg.validate().is_ok());
}

// ── Request construction with credentials ────────────────────────────────────

#[test]
fn full_outbound_request_carries_all_configured_headers() {
    let signing_key = b"webhook-hmac-key";
    let opts = OutboundRequestOptions::with_idempotency_key("txn-606")
        .with_signing_key(signing_key)
        .with_bearer_token("sep10-jwt");
    assert!(opts.validate().is_ok());

    let body = r#"{"event":"deposit_completed"}"#;
    let mut captured: Vec<(String, String)> = Vec::new();
    let status = post_with_options(
        "https://anchor.example.com/webhook",
        body,
        Some(&opts),
        |_url, _body, hdrs| {
            captured.extend(hdrs.iter().cloned());
            Ok(200u16)
        },
    );
    assert_eq!(status, Ok(200));

    let get = |name: &str| {
        captured
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("Idempotency-Key"), Some("txn-606"));
    assert_eq!(get("X-Request-Id"), Some("txn-606"));
    assert_eq!(get("Authorization"), Some("Bearer sep10-jwt"));
    let signature = get("X-Anchor-Signature").expect("signature header present");
    assert!(
        verify_outbound_signature(body, signature, signing_key),
        "emitted signature must verify against the body"
    );
}

#[test]
fn basic_auth_credentials_encode_correctly() {
    let opts = OutboundRequestOptions::default().with_basic_auth("user", "pass");
    let headers = opts.build_headers("");
    // base64("user:pass") == "dXNlcjpwYXNz"
    assert_eq!(
        headers,
        vec![("Authorization".to_string(), "Basic dXNlcjpwYXNz".to_string())]
    );
}

#[test]
fn custom_header_credentials_are_injected() {
    let opts = OutboundRequestOptions::default().with_credentials(RequestCredentials::Header {
        name: "X-Api-Key".to_string(),
        value: "key-123".to_string(),
    });
    let headers = opts.build_headers("");
    assert_eq!(
        headers,
        vec![("X-Api-Key".to_string(), "key-123".to_string())]
    );
}

// ── Invalid configurations are rejected ──────────────────────────────────────

#[test]
fn invalid_proxy_configurations_are_rejected() {
    let invalid = [
        ProxyConfig {
            proxy_url: Some("socks5://proxy.corp:1080".to_string()),
            ..ProxyConfig::default()
        },
        ProxyConfig {
            http_proxy_url: Some("http://".to_string()),
            ..ProxyConfig::default()
        },
        ProxyConfig {
            https_proxy_url: Some("http://proxy.corp:3128 x".to_string()),
            ..ProxyConfig::default()
        },
        ProxyConfig {
            credentials: Some(ProxyCredentials {
                username: "svc".to_string(),
                password: "pw".to_string(),
            }),
            ..ProxyConfig::default()
        },
        ProxyConfig {
            proxy_url: Some("http://proxy.corp:3128".to_string()),
            credentials: Some(ProxyCredentials {
                username: String::new(),
                password: "pw".to_string(),
            }),
            ..ProxyConfig::default()
        },
        ProxyConfig {
            proxy_url: Some("http://proxy.corp:3128".to_string()),
            credentials: Some(ProxyCredentials {
                username: "user:name".to_string(),
                password: "pw".to_string(),
            }),
            ..ProxyConfig::default()
        },
    ];
    for (idx, cfg) in invalid.iter().enumerate() {
        assert!(
            cfg.validate().is_err(),
            "invalid configuration #{idx} should be rejected: {cfg:?}"
        );
    }
}

#[test]
fn invalid_request_credentials_are_rejected() {
    let invalid = [
        RequestCredentials::Bearer(String::new()),
        RequestCredentials::Bearer("tok\r\nInjected: 1".to_string()),
        RequestCredentials::Basic {
            username: "a:b".to_string(),
            password: "pw".to_string(),
        },
        RequestCredentials::Header {
            name: "Bad Name".to_string(),
            value: "v".to_string(),
        },
    ];
    for creds in &invalid {
        assert!(creds.validate().is_err(), "should be rejected: {creds:?}");
    }
}

// ── Secrets stay out of Debug output ─────────────────────────────────────────

#[test]
fn secrets_are_redacted_everywhere() {
    let cfg = corp_proxy_config();
    let opts = OutboundRequestOptions::with_idempotency_key("idem")
        .with_signing_key(b"hmac-secret-key")
        .with_credentials(RequestCredentials::Basic {
            username: "endpoint-user".to_string(),
            password: "endpoint-pw".to_string(),
        });

    let shown = format!("{cfg:?} {opts:?}");
    for secret in ["proxy-pw", "endpoint-pw", "hmac-secret-key"] {
        assert!(
            !shown.contains(secret),
            "secret '{secret}' leaked into Debug output: {shown}"
        );
    }
    // Non-secret identifiers stay visible for debugging.
    assert!(shown.contains("svc-anchor"));
    assert!(shown.contains("endpoint-user"));
    assert!(shown.contains("idem"));
}

// ── std-gated: real client construction ──────────────────────────────────────

#[cfg(feature = "std")]
mod std_client {
    use super::*;
    use anchorkit::http_client::{build_client, build_client_with_policy, ConnectionPolicy};

    #[test]
    fn client_builds_with_proxies_and_credentials() {
        let client = build_client(Some(&corp_proxy_config()), 10);
        assert!(client.is_ok(), "should build: {:?}", client.err());
        let client = build_client_with_policy(Some(&corp_proxy_config()), &ConnectionPolicy::default());
        assert!(client.is_ok());
    }

    #[test]
    fn client_rejects_invalid_configuration() {
        let cfg = ProxyConfig {
            credentials: Some(ProxyCredentials {
                username: "svc".to_string(),
                password: "pw".to_string(),
            }),
            ..ProxyConfig::default()
        };
        assert!(build_client(Some(&cfg), 10).is_err());
    }
}
