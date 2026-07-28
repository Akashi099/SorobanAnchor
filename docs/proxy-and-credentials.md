# Proxy Routing and Credential Handling

AnchorKit routes every outbound HTTP request (stellar.toml discovery, webhook
delivery, SEP-6 status checks) through a single, centrally configured HTTP
client. This document describes how to configure proxies and credentials for
enterprise deployments (issue #606).

## Proxy configuration

Proxy settings live in the optional `proxy` section of the runtime
configuration file, or can be built programmatically as
`anchorkit::ProxyConfig`.

```json
{
  "proxy": {
    "proxy_url": "http://proxy.corp.example.com:3128",
    "http_proxy_url": null,
    "https_proxy_url": "http://tls-proxy.corp.example.com:3129",
    "no_proxy": "localhost,127.0.0.1,.internal.example.com",
    "credentials": { "username": "svc-anchor", "password": "s3cret" }
  }
}
```

```toml
[proxy]
proxy_url = "http://proxy.corp.example.com:3128"
https_proxy_url = "http://tls-proxy.corp.example.com:3129"
no_proxy = "localhost,127.0.0.1,.internal.example.com"

[proxy.credentials]
username = "svc-anchor"
password = "s3cret"
```

| Field | Meaning |
|---|---|
| `proxy_url` | Catch-all proxy for every outbound request. |
| `http_proxy_url` | Proxy used only for plain-HTTP targets; overrides `proxy_url` for those requests. |
| `https_proxy_url` | Proxy used only for HTTPS targets; overrides `proxy_url` for those requests. |
| `no_proxy` | Comma-separated bypass list (see below). |
| `credentials` | Optional HTTP Basic credentials sent to the proxy (`Proxy-Authorization`). Applied to every configured proxy URL. |

All URL fields must start with `http://` or `https://`. An empty string is
treated the same as omitting the field. When no proxy URL is configured the
client falls back to the standard `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY`
environment variables.

### Selection rules

For each request the effective proxy is chosen as follows:

1. If the target host matches `no_proxy`, connect directly.
2. Otherwise use the scheme-specific proxy (`https_proxy_url` for `https://`
   targets, `http_proxy_url` for `http://` targets) when configured.
3. Otherwise use `proxy_url` when configured.
4. Otherwise connect directly (or via environment-variable proxies).

`ProxyConfig::select_proxy_url(target_url)` exposes this decision as a pure
function so deployments can unit-test and log routing decisions without
opening connections.

### `no_proxy` matching

- `*` bypasses the proxy for every host.
- An entry starting with a dot (`.internal.example.com`) matches the domain
  itself and any subdomain.
- A bare host (`example.com`, `127.0.0.1`, `::1`) matches itself and any
  subdomain.
- Matching is case-insensitive and ignores the target port. CIDR ranges are
  honoured by the underlying transport but compared textually by
  `select_proxy_url`.

### Validation

`ProxyConfig::validate()` runs automatically when a runtime config file is
loaded and when a client is built. Rejected configurations include:

- proxy URLs with a non-HTTP scheme, missing host, or embedded whitespace /
  control characters;
- credentials with an empty username or a username containing `:`;
- credentials containing control characters (header-injection guard);
- credentials supplied without any proxy URL (they would silently never be
  used);
- unknown fields in the `proxy` section (typo guard).

## Request credentials

Independent of proxy authentication, outbound requests to the *target*
endpoint can carry credentials via `OutboundRequestOptions`:

```rust
use anchorkit::http_client::{OutboundRequestOptions, RequestCredentials};

// SEP-10 JWT bearer token:
let opts = OutboundRequestOptions::with_idempotency_key("txn-001")
    .with_bearer_token("sep10-jwt");

// HTTP Basic:
let opts = OutboundRequestOptions::default().with_basic_auth("user", "pass");

// Non-standard schemes, e.g. API keys:
let opts = OutboundRequestOptions::default()
    .with_credentials(RequestCredentials::Header {
        name: "X-Api-Key".into(),
        value: "key-123".into(),
    });
```

`build_headers` then emits the matching header (`Authorization: Bearer …`,
`Authorization: Basic <base64>`, or the custom header) alongside the existing
idempotency and HMAC-signature headers, for both the injectable-transport path
(`post_with_options`) and the reqwest-backed helpers.

`OutboundRequestOptions::validate()` / `RequestCredentials::validate()` reject
empty tokens, usernames containing `:`, malformed header names, and CR/LF in
any credential value.

## Handling credentials safely

- **Debug redaction** — `ProxyCredentials`, `RequestCredentials`, and
  `OutboundRequestOptions` implement `Debug` manually: passwords, tokens, and
  signing keys are printed as `<redacted>`. Usernames and header names stay
  visible for debugging.
- **Zeroization** — credential structs are zeroized on drop (via `zeroize`),
  limiting the window plaintext secrets live in memory.
- **Prefer the `credentials` field over URL userinfo** — a
  `http://user:pass@proxy` URL works, but URLs are echoed verbatim into error
  messages and logs. The dedicated credentials field is never echoed.
- **Header-injection guard** — control characters (CR/LF) are rejected in
  every credential value at validation time.
