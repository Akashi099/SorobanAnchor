#!/usr/bin/env bash
# Full Operator Walkthrough (#638)
#
# An end-to-end tour of the CLI commands an operator runs to take AnchorKit
# from "nothing deployed" to "attestor registered and attesting on testnet",
# plus the commands to reach for when something goes wrong.
#
# This script prints the commands and explains what to expect — it does not
# execute them, so it's safe to read top-to-bottom or run directly to see the
# whole flow without touching a live network.
#
# Run:
#   bash examples/full_deployment_walkthrough.sh

set -e

GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

step() { echo -e "${BLUE}$1${NC}"; echo "--------------------------------------"; }
ok()   { echo -e "  ${GREEN}✓ $1${NC}"; echo ""; }
warn() { echo -e "  ${YELLOW}⚠ $1${NC}"; }

echo "=== AnchorKit Operator Walkthrough ==="
echo ""

# ── Step 0: Environment check ──────────────────────────────────────────────
step "Step 0: Check your environment before doing anything else"
echo "  anchorkit doctor"
echo ""
echo "  Verifies: Stellar CLI version, wasm32 target, ANCHOR_CONTRACT_ID /"
echo "  ANCHOR_ADMIN_SECRET presence, network reachability, and (once deployed)"
echo "  that the contract responds and its endpoint PoP records are verified."
echo ""
echo "  Fix what you can automatically:"
echo "    anchorkit doctor --fix"
ok "Doctor tells you exactly what's missing before you spend a transaction fee"

# ── Step 1: Validate config offline ──────────────────────────────────────────
step "Step 1: Validate your config files before deploying (no network needed)"
echo "  anchorkit offline validate --config configs/remittance-anchor.toml"
echo ""
echo "  Or validate everything under configs/:"
echo "    anchorkit offline validate"
echo ""
echo "  Dry-run a workflow without touching the network:"
echo "    anchorkit offline simulate --workflow deploy"
ok "Config errors are caught before you pay for a failed transaction"

# ── Step 2: Deploy ────────────────────────────────────────────────────────────
step "Step 2: Deploy the contract"
echo "  anchorkit deploy --network testnet --source anchor-admin"
echo ""
echo "  What happens:"
echo "    1. Pre-deployment validation (WASM artifact, config files, network reachability)"
echo "    2. cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm"
echo "    3. stellar contract deploy"
echo "    4. Automatic initialize(admin) call"
echo "    5. Deployment record saved to .anchorkit/deployments.json"
echo ""
warn "Deploying to mainnet prompts for confirmation unless you pass --yes"
echo "    anchorkit deploy --network mainnet --source anchor-admin --yes"
echo ""
echo "  Preview deployment history any time:"
echo "    anchorkit deploy --list"
ok "Contract ID printed and saved — export it for later commands"
echo "    export ANCHOR_CONTRACT_ID=<printed contract id>"
echo ""

# ── Step 3: Register an attestor ─────────────────────────────────────────────
step "Step 3: Register an attestor"
echo "  anchorkit register \\"
echo "    --address GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ \\"
echo "    --services deposits,withdrawals,kyc \\"
echo "    --sep10-token \$SEP10_JWT \\"
echo "    --sep10-issuer GISSUER..."
echo ""
echo "  --services must name at least one of: deposits, withdrawals, quotes, kyc"
echo "  (an empty --services list is now rejected with a clear error before any"
echo "  network call is made)."
ok "Attestor registered and its services configured in one command"

# ── Step 4: Submit an attestation ────────────────────────────────────────────
step "Step 4: Submit an attestation"
echo "  PAYLOAD_HASH=\$(echo -n 'deposit:usdc:500' | sha256sum | awk '{print \$1}')"
echo "  anchorkit attest \\"
echo "    --subject GUSER... --payload-hash \$PAYLOAD_HASH \\"
echo "    --issuer GISSUER... --credential-name kyc-attestor-key"
echo ""
echo "  See examples/attestation_workflow.sh for session-batched and"
echo "  request-ID-traced variants."
ok "Attestation ID printed — save it for verification"

# ── Step 5: Verify and check health ──────────────────────────────────────────
step "Step 5: Verify the attestation and check contract health"
echo "  anchorkit verify --id <ATTESTATION_ID>"
echo "  anchorkit health --contract-id \$ANCHOR_CONTRACT_ID --attestor GBBD6A7..."
ok "Confirms the record landed on-chain and the attestor isn't rate-limited"

# ── Step 6: Troubleshooting ──────────────────────────────────────────────────
step "Step 6: When something goes wrong"
echo "  Symptom: 'error: --contract-id (or ANCHOR_CONTRACT_ID) is required'"
echo "    → export ANCHOR_CONTRACT_ID=<id>  or pass --contract-id explicitly"
echo ""
echo "  Symptom: 'signing key required'"
echo "    → resolution order: --ephemeral-token > --secret-key >"
echo "      ANCHOR_ADMIN_SECRET > --keypair-file > --credential-name"
echo "      Pick exactly one; anchorkit doctor reports which are set."
echo ""
echo "  Symptom: 'invalid --admin address'"
echo "    → --admin must be the literal 'default' or a 56-char Stellar public"
echo "      address starting with 'G' (not a secret key, not a contract ID)."
echo ""
echo "  Symptom: attestation rejected with RateLimitExceeded"
echo "    → anchorkit health --attestor <ADDR> shows throttled state."
echo "      See examples/rate_limit_override_example.sh to grant a per-attestor override."
echo ""
echo "  Symptom: config validation fails on deploy"
echo "    → anchorkit offline validate --config <path> isolates the exact file/error"
echo "      without needing network access or a signing key."
echo ""
echo "  Symptom: unsure what changed in a live config file"
echo "    → see examples/config_hot_reload_example.rs for reloading a running"
echo "      process's config without a restart."
ok "Doctor + offline validate + health cover the vast majority of operator issues"

echo "=== Summary ==="
echo "  doctor → offline validate → deploy → register → attest → verify → health"
echo ""
echo "  Further reading:"
echo "    docs/RUNBOOK.md"
echo "    docs/governance-and-security.md"
echo "    examples/credential_management.sh (secret storage options)"
echo "    examples/mock_mode_example.sh (testing without a live anchor)"
