# Attestor Roles and Permissions

This document provides a comprehensive guide to the attestor roles supported by SorobanAnchor and their expected permissions and responsibilities.

## Overview

Attestors are trusted entities that provide cryptographic attestations for various operations within the anchor ecosystem. Each attestor is assigned a specific role that determines what types of operations they can attest to and what permissions they have within the system.

## The `enabled` Flag

Every entry in `attestors.registry` carries a required `enabled` boolean field.

### What `enabled` controls

The `enabled` flag is a **configuration-layer marker** used to indicate
whether an attestor should participate in normal workflows.  Its effect at
each layer is:

| Layer | Behaviour |
|-------|-----------|
| Config schema | `enabled` is **required** and must be a boolean. Omitting it causes `offline validate` to fail. |
| Config loading (`parse_runtime_config_str`) | The field is parsed and stored as `AttestorConfig.enabled: bool`. No entries are dropped during loading — both `true` and `false` entries are retained in the parsed registry. |
| CLI deploy / register path | The CLI does not filter entries by `enabled`. All registry entries are loaded equally; none are skipped or rejected based on this flag. |
| Validation and error messages | A disabled entry does not trigger a validation error or a warning. Its fields are validated exactly like those of an enabled entry. |

**Key point:** setting `enabled: false` does **not** prevent the entry from
appearing in the parsed config, and it does not cause the CLI to skip that
attestor when it reads the registry.  The flag is a human-readable signal for
staged rollouts and future tooling — it does not currently affect runtime
selection.

### When to set `enabled: false`

Use `enabled: false` to document an attestor that has been onboarded to the
config file but should not yet be active.  This pattern is useful for:

- Staged rollouts: commit the attestor to config review before activating it.
- Graceful off-boarding: mark an attestor as disabled before removing it so
  reviewers have a visible change history.
- Environment-specific overrides: keep a disabled entry as a clear placeholder
  for a provider that is not yet available in a given environment.

### Snippet: disabled attestor in a config fragment

The following is a valid, schema-conforming TOML config fragment showing a
disabled attestor alongside an active one.  The offline validator accepts both
entries; neither entry produces a warning.

```toml
[attestors]

[[attestors.registry]]
name        = "kyc-provider"
address     = "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ"
description = "Primary KYC verification service"
endpoint    = "https://kyc-provider.example.com/verify"
role        = "kyc-issuer"
enabled     = true

# This attestor has been onboarded to the config for review but is not yet
# active.  Setting enabled = false documents the staged-rollout intent.
# The entry passes schema validation and is visible in the parsed registry,
# but it is not currently selected by the runtime for any workflow.
[[attestors.registry]]
name        = "kyc-provider-backup"
address     = "GBZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
description = "Backup KYC service — staged rollout, not yet active"
endpoint    = "https://kyc-backup.example.com/verify"
role        = "kyc-issuer"
enabled     = false
```

The equivalent JSON fragment:

```json
{
  "attestors": {
    "registry": [
      {
        "name": "kyc-provider",
        "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
        "description": "Primary KYC verification service",
        "endpoint": "https://kyc-provider.example.com/verify",
        "role": "kyc-issuer",
        "enabled": true
      },
      {
        "name": "kyc-provider-backup",
        "address": "GBZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
        "description": "Backup KYC service — staged rollout, not yet active",
        "endpoint": "https://kyc-backup.example.com/verify",
        "role": "kyc-issuer",
        "enabled": false
      }
    ]
  }
}
```

Both fragments pass `anchorkit offline validate` without errors or warnings.

### Validating the snippet

Run the offline validator to confirm the fragment is schema-conforming before
committing:

```bash
anchorkit offline validate --config configs/my-anchor.toml
```

A passing run prints `✓ configs/my-anchor.toml` and exits 0.  No network
access is required.

---

## Role Definitions

### 1. kyc-issuer

**Purpose**: Issues Know Your Customer (KYC) verification attestations for users.

**Permissions**:
- Issue KYC verification attestations
- Update KYC status (pending, approved, rejected)
- Specify KYC verification levels (basic, intermediate, advanced)
- Record verification timestamps and methods

**Typical Operations**:
- `kyc_verification`: Attest that a user has completed KYC at a specified level
- Identity document verification
- Biometric verification
- Address verification

**Use Cases**:
- Fiat on/off-ramp services requiring user identity verification
- Compliance with regulatory requirements (AML/KYC)
- Risk-based customer due diligence

**Example Configuration**:
```json
{
  "name": "kyc-provider",
  "address": "GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ",
  "role": "kyc-issuer",
  "enabled": true,
  "endpoint": "https://kyc-provider.example.com/verify"
}
```

---

### 2. transfer-verifier

**Purpose**: Verifies and attests to fund transfers between financial institutions.

**Permissions**:
- Verify incoming fund transfers
- Confirm transfer completion
- Attest to transfer amounts and currencies
- Record transaction references

**Typical Operations**:
- `fund_transfer`: Confirm receipt of fiat deposits
- `fund_refund`: Process refunds for failed transactions
- Bank wire transfer confirmations
- Payment gateway verifications

**Use Cases**:
- Bank integration for deposit confirmations
- Payment processor attestations
- Cross-border payment verification

**Example Configuration**:
```json
{
  "name": "bank-integration",
  "address": "GBXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "role": "transfer-verifier",
  "enabled": true,
  "endpoint": "https://banking-api.example.com/transfers"
}
```

---

### 3. compliance-approver

**Purpose**: Provides manual compliance review and approval for high-risk or flagged transactions.

**Permissions**:
- Approve or reject withdrawal requests
- Review flagged transactions
- Override automated compliance decisions
- Document compliance review decisions

**Typical Operations**:
- `withdrawal_approval`: Approve withdrawal requests after manual review
- `compliance_override`: Override automated compliance flags
- High-value transaction approvals
- Suspicious activity review

**Use Cases**:
- Manual review of high-value transactions
- Compliance officer approvals
- Enhanced due diligence workflows
- Sanctions screening overrides

**Example Configuration**:
```json
{
  "name": "compliance-officer",
  "address": "GBCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWXYZAB",
  "role": "compliance-approver",
  "enabled": true
}
```

---

### 4. rate-provider

**Purpose**: Provides exchange rate attestations for currency conversions.

**Permissions**:
- Publish exchange rates
- Lock rates for specific time periods
- Attest to rate sources and timestamps
- Provide rate variance tolerances

**Typical Operations**:
- `exchange_rate_quote`: Provide exchange rate for currency pairs
- `rate_lock`: Lock exchange rate for transaction duration
- Rate feed updates
- Historical rate attestations

**Use Cases**:
- Remittance corridor exchange rates
- Multi-currency stablecoin operations
- Cross-border payment pricing
- Rate transparency and auditability

**Example Configuration**:
```json
{
  "name": "fx-rate-provider",
  "address": "GBRATEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "role": "rate-provider",
  "enabled": true,
  "endpoint": "https://rates.example.com/api"
}
```

---

### 5. attestor

**Purpose**: Generic attestor role for custom or multi-purpose attestation services.

**Permissions**:
- Issue general-purpose attestations
- Flexible operation types based on configuration
- Custom payload schemas
- Multi-domain attestations

**Typical Operations**:
- Custom attestation types defined in operation templates
- Multi-purpose verification services
- Experimental or prototype attestations

**Use Cases**:
- Development and testing environments
- Custom business logic attestations
- Third-party service integrations
- Flexible attestation workflows

**Example Configuration**:
```json
{
  "name": "general-attestor",
  "address": "GBATTESTXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "role": "attestor",
  "enabled": true
}
```

---

### 6. identity-verifier

**Purpose**: Verifies user identities through biometric and document verification for remittance operations.

**Permissions**:
- Verify sender identities
- Verify recipient identities
- Validate identity documents (passport, national ID, driver's license)
- Perform biometric verification
- Hash and attest to account details

**Typical Operations**:
- `sender_verification`: Verify remittance sender identity
- `recipient_verification`: Verify remittance recipient identity
- Document authenticity checks
- Biometric matching

**Use Cases**:
- Remittance sender/recipient verification
- Cross-border payment identity checks
- Enhanced identity verification for high-value transfers
- Beneficiary verification

**Example Configuration**:
```json
{
  "name": "identity-verifier",
  "address": "GBAA5XKQC3KVDPD5OS3CHJJ24SB3BX7GI7XBXKNNCKQVPQVX6S3VT5O",
  "role": "identity-verifier",
  "enabled": true,
  "endpoint": "https://identity-service.example.com/verify"
}
```

---

### 7. settlement-bank

**Purpose**: Manages settlement operations with correspondent banks for remittance corridors.

**Permissions**:
- Issue settlement instructions
- Confirm settlement completion
- Attest to final settlement amounts
- Provide settlement references and timestamps

**Typical Operations**:
- `settlement_instruction`: Send settlement instructions to correspondent bank
- `settlement_confirmation`: Confirm funds settled to recipient
- Settlement status updates
- Settlement reconciliation

**Use Cases**:
- Correspondent banking relationships
- Cross-border settlement operations
- Remittance corridor management
- Final mile delivery confirmation

**Example Configuration**:
```json
{
  "name": "correspondent-bank",
  "address": "GBXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "role": "settlement-bank",
  "enabled": true,
  "endpoint": "https://bank-api.example.com/settlement"
}
```

---

### 8. corridor-manager

**Purpose**: Manages remittance corridors and routing decisions for cross-border payments.

**Permissions**:
- Select optimal remittance routes
- Manage corridor configurations
- Attest to routing decisions
- Monitor corridor performance

**Typical Operations**:
- `corridor_routing`: Select optimal route for remittance
- Corridor performance monitoring
- Route optimization
- Corridor availability management

**Use Cases**:
- Remittance corridor optimization
- Multi-route remittance networks
- Dynamic routing based on cost/speed
- Corridor partnership management

**Example Configuration**:
```json
{
  "name": "corridor-operator",
  "address": "GBYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY",
  "role": "corridor-manager",
  "enabled": true
}
```

---

### 9. compliance-checker

**Purpose**: Performs automated compliance checks including AML/CFT screening and sanctions list verification.

**Permissions**:
- Screen transactions against sanctions lists
- Perform AML/CFT compliance checks
- Issue clearance or block decisions
- Flag transactions for manual review

**Typical Operations**:
- `aml_screening`: Screen transaction against sanctions lists
- `sanctions_check`: Verify parties against restricted lists
- Compliance clearance attestations
- Risk scoring

**Use Cases**:
- Automated AML/CFT screening
- OFAC and sanctions list checking
- Transaction monitoring
- Regulatory compliance automation

**Example Configuration**:
```json
{
  "name": "aml-screening",
  "address": "GBZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
  "role": "compliance-checker",
  "enabled": true,
  "endpoint": "https://aml-service.example.com/screen"
}
```

---

### 10. reserve-verifier

**Purpose**: Audits and verifies reserve adequacy for stablecoin issuers.

**Permissions**:
- Audit reserve holdings
- Attest to reserve ratios
- Verify currency composition
- Issue reserve adequacy reports

**Typical Operations**:
- `reserve_attestation`: Confirm sufficient reserves backing stablecoins
- Reserve composition verification
- Reserve ratio calculations
- Periodic audit attestations

**Use Cases**:
- Stablecoin reserve auditing
- Transparency and trust building
- Regulatory compliance for stablecoins
- Reserve adequacy monitoring

**Example Configuration**:
```json
{
  "name": "reserve-auditor",
  "address": "GBAA5XKQC3KVDPD5OS3CHJJ24SB3BX7GI7XBXKNNCKQVPQVX6S3VT5O",
  "role": "reserve-verifier",
  "enabled": true,
  "endpoint": "https://auditor.example.com/reserves"
}
```

---

### 11. collateral-custodian

**Purpose**: Manages collateral deposits, withdrawals, and liquidations for collateralized stablecoins.

**Permissions**:
- Accept and custody collateral deposits
- Execute collateral liquidations
- Monitor collateral ratios
- Release collateral upon redemption

**Typical Operations**:
- `collateral_deposit`: Register collateral deposits
- `collateral_liquidation`: Execute liquidations when ratios fall below thresholds
- `collateral_withdrawal`: Process collateral withdrawals
- Collateral valuation updates

**Use Cases**:
- Collateralized stablecoin operations
- Over-collateralized lending protocols
- Liquidation management
- Collateral custody services

**Example Configuration**:
```json
{
  "name": "collateral-manager",
  "address": "GBXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
  "role": "collateral-custodian",
  "enabled": true,
  "endpoint": "https://collateral-mgmt.example.com/positions"
}
```

---

### 12. treasury-operator

**Purpose**: Authorizes and executes treasury operations including minting, burning, and redemptions.

**Permissions**:
- Authorize minting of new stablecoins
- Confirm burning of stablecoins
- Process redemption requests
- Manage stablecoin supply

**Typical Operations**:
- `mint_authorization`: Authorize minting of new tokens
- `burn_confirmation`: Confirm token burning
- `redemption_processing`: Process redemptions for underlying reserves
- Supply management operations

**Use Cases**:
- Stablecoin issuance and redemption
- Supply management
- Treasury operations
- Reserve-backed token operations

**Example Configuration**:
```json
{
  "name": "treasury-manager",
  "address": "GBYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY",
  "role": "treasury-operator",
  "enabled": true
}
```

---

### 13. risk-analyst

**Purpose**: Monitors and assesses risk parameters for stablecoin and DeFi operations.

**Permissions**:
- Monitor collateral ratios
- Assess liquidation risks
- Provide price feed attestations
- Calculate risk scores

**Typical Operations**:
- `risk_assessment`: Monitor and update risk parameters
- `price_feed`: Provide price feed attestations for collateral assets
- Risk scoring and monitoring
- Liquidation impact analysis

**Use Cases**:
- Continuous risk monitoring
- Price feed provision
- Liquidation risk assessment
- Market risk analysis

**Example Configuration**:
```json
{
  "name": "risk-monitor",
  "address": "GBZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
  "role": "risk-analyst",
  "enabled": true,
  "endpoint": "https://risk-system.example.com/monitor"
}
```

---

## Permission Matrix

| Role | KYC | Transfers | Compliance | Rates | Identity | Settlement | Routing | Reserves | Collateral | Treasury | Risk |
|------|-----|-----------|------------|-------|----------|------------|---------|----------|------------|----------|------|
| kyc-issuer | ✓ | | | | | | | | | | |
| transfer-verifier | | ✓ | | | | | | | | | |
| compliance-approver | | | ✓ | | | | | | | | |
| rate-provider | | | | ✓ | | | | | | | |
| attestor | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| identity-verifier | | | | | ✓ | | | | | | |
| settlement-bank | | | | | | ✓ | | | | | |
| corridor-manager | | | | | | | ✓ | | | | |
| compliance-checker | | | ✓ | | | | | | | | |
| reserve-verifier | | | | | | | | ✓ | | | |
| collateral-custodian | | | | | | | | | ✓ | | |
| treasury-operator | | | | | | | | | | ✓ | |
| risk-analyst | | | | | | | | | | | ✓ |

## Security Considerations

### Multi-Signature Requirements

Critical operations should require multiple attestor signatures. Configure multi-signature requirements in the `security.multisig_requirements` section:

```json
{
  "security": {
    "multisig_requirements": [
      {
        "operation": "mint",
        "required_signatures": 2,
        "signatory_attestors": ["treasury-manager"]
      },
      {
        "operation": "reserve-proof",
        "required_signatures": 1,
        "signatory_attestors": ["reserve-auditor"]
      }
    ]
  }
}
```

### Rate Limiting

Each attestor role should have appropriate rate limits to prevent abuse:

```json
{
  "security": {
    "rate_limits": [
      {
        "attestor": "kyc-provider",
        "requests_per_minute": 100,
        "requests_per_hour": 5000
      }
    ]
  }
}
```

### Signature Verification

All attestations must be cryptographically signed by the attestor's registered address. Enable signature verification:

```json
{
  "security": {
    "require_signature_verification": true,
    "signature_algorithm": "ed25519",
    "signature_expiry_seconds": 300,
    "nonce_required": true,
    "nonce_reuse_prevention": true
  }
}
```

## Role Assignment Best Practices

1. **Principle of Least Privilege**: Assign the most specific role that matches the attestor's function. Avoid using the generic `attestor` role in production.

2. **Separation of Duties**: Different entities should handle different roles. For example, the same entity should not be both `compliance-approver` and `transfer-verifier`.

3. **Role Verification**: Verify that attestors have the appropriate credentials, licenses, and capabilities before assigning roles.

4. **Regular Audits**: Periodically review attestor roles and permissions to ensure they remain appropriate.

5. **Role Documentation**: Document why each attestor was assigned their specific role and what operations they are expected to perform.

## Operation Templates and Role Mapping

Operation templates in the configuration must reference valid attestor names. The system validates that each operation's attestor exists in the registry:

```json
{
  "operations": {
    "templates": [
      {
        "id": "kyc-verification",
        "name": "KYC Verification",
        "attestor": "kyc-provider",
        "operation_type": "kyc_verification",
        "required_fields": ["user_id", "kyc_status"],
        "replay_protection": "enabled"
      }
    ]
  }
}
```

The attestor name must match an entry in `attestors.registry`, and the operation type should align with the attestor's role capabilities.

## Monitoring and Alerts

Configure monitoring alerts for role-specific events:

```json
{
  "monitoring": {
    "alerts": [
      {
        "condition": "failed_kyc_verification",
        "severity": "warning",
        "recipients": ["ops@example.com"]
      },
      {
        "condition": "unauthorized_mint_attempt",
        "severity": "critical",
        "recipients": ["security@example.com"]
      }
    ]
  }
}
```

## Example Configurations by Use Case

### Fiat On/Off-Ramp
Required roles:
- `kyc-issuer`: User verification
- `transfer-verifier`: Bank transfer confirmation
- `compliance-approver`: Manual review for high-risk transactions

See: `configs/fiat-on-off-ramp.json`

### Remittance Anchor
Required roles:
- `identity-verifier`: Sender/recipient verification
- `settlement-bank`: Correspondent bank settlement
- `corridor-manager`: Route optimization
- `compliance-checker`: AML/sanctions screening

See: `configs/remittance-anchor.json`

### Stablecoin Issuer
Required roles:
- `reserve-verifier`: Reserve auditing
- `collateral-custodian`: Collateral management
- `treasury-operator`: Mint/burn operations
- `risk-analyst`: Risk monitoring and price feeds

See: `configs/stablecoin-issuer.json`

## Unified AttestorProfile Storage

All attestor metadata (endpoint, webhook URL, services) is stored in a single `AttestorProfile` struct under the `(symbol_short!("PROFILE"), attestor)` key, rather than separate raw storage keys. This ensures consistency across reads and writes.

**Relevant functions:**
- `set_endpoint` / `register_webhook` — write to `AttestorProfile` via `save_profile`
- `get_endpoint` / `get_webhook_url` — read from `AttestorProfile` via `load_or_init_profile`
- `configure_services_versioned` — syncs services into `AttestorProfile` after storing under `SERVICES` key

**Contract-Side Role Enforcement**

While the configuration defines roles, the actual permission enforcement happens at the contract level. The contract should:

1. **Verify Attestor Registration**: Check that the attestor address is registered in the attestor registry
2. **Validate Role Assignment**: Verify the attestor has the appropriate role for the operation
3. **Check Signature**: Cryptographically verify the attestation signature
4. **Enforce Rate Limits**: Apply per-attestor rate limits
5. **Require Multi-Sig**: Enforce multi-signature requirements for critical operations
6. **Prevent Replay**: Use nonces and timestamps to prevent replay attacks

## Adding Custom Roles

To add a custom role:

1. Update `config_schema.json` to include the new role in the `role` enum
2. Document the role's permissions and use cases in this file
3. Update the permission matrix
4. Add example configurations demonstrating the role
5. Update contract logic to handle the new role if needed

## References

- Configuration Schema: `config_schema.json`
- Example Configurations: `configs/`
- Configuration Validation: `src/config.rs`
- Error Codes: `docs/error-codes.md`
- Security: `docs/secret-file-encryption.md`
