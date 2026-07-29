#!/usr/bin/env bash
# Per-Tenant / Per-Attestor Rate Limit Override Example (#631)
#
# Covers: setting the global default rate limit, overriding it per role
# (tenant), overriding it per individual attestor address (which takes
# precedence over both the role override and the global default), and
# reading overrides back.
#
# Prerequisites:
#   - stellar CLI installed and configured with a signing identity named
#     "anchor-admin" that holds the contract's admin key (or the
#     SetRateLimits capability — see docs/governance-and-security.md)
#   - ANCHOR_CONTRACT_ID env var set
#   - STELLAR_NETWORK env var set (default: testnet)
#
# Run:
#   bash examples/rate_limit_override_example.sh

set -e

CONTRACT_ID="${ANCHOR_CONTRACT_ID:-CBXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX}"
NETWORK="${STELLAR_NETWORK:-testnet}"
ADMIN_ACCOUNT="anchor-admin"
HIGH_VOLUME_ATTESTOR="GBBD6A7KNZF5WNWQEPZP5DYJD2AYUTLXRB6VXJ4RCX4RTNPPQVNF3GQ"

GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

echo "=== Rate Limit Override Example ==="
echo "Network:     $NETWORK"
echo "Contract ID: $CONTRACT_ID"
echo ""

# ── Step 1: Set the global default ─────────────────────────────────────────
echo -e "${BLUE}Step 1: Set the global default rate limit${NC}"
echo "--------------------------------------"
echo "  10 submissions per 100-ledger window applies to every attestor by default."
echo ""
echo "  stellar contract invoke \\"
echo "    --id $CONTRACT_ID --source $ADMIN_ACCOUNT \\"
echo "    --rpc-url \$RPC_URL --network-passphrase \"\$NETWORK_PASSPHRASE\" \\"
echo "    -- set_rate_limit_config \\"
echo "    --caller $ADMIN_ACCOUNT --max_submissions 10 --window_length 100"
echo ""
echo -e "  ${GREEN}✓ Global default set${NC}"
echo ""

# ── Step 2: Override for a whole role/tenant ────────────────────────────────
echo -e "${BLUE}Step 2: Override the limit for a role (tenant)${NC}"
echo "--------------------------------------"
echo "  Every attestor holding the KycAdmin role gets a tighter window."
echo ""
echo "  stellar contract invoke \\"
echo "    --id $CONTRACT_ID --source $ADMIN_ACCOUNT \\"
echo "    --rpc-url \$RPC_URL --network-passphrase \"\$NETWORK_PASSPHRASE\" \\"
echo "    -- set_role_rate_limit \\"
echo "    --caller $ADMIN_ACCOUNT --role KycAdmin \\"
echo "    --config '{\"max_submissions\":3,\"window_length\":20}'"
echo ""
echo "  Rust API:"
cat <<'RUST'
    AnchorKitContract::set_role_rate_limit(
        env, admin, Symbol::new(&env, "KycAdmin"),
        RateLimitConfig { max_submissions: 3, window_length: 20 },
    );
RUST
echo ""
echo -e "  ${GREEN}✓ Role override active — takes precedence over the global default${NC}"
echo ""

# ── Step 3: Override for a single high-value attestor ──────────────────────
echo -e "${BLUE}Step 3: Override the limit for one attestor address${NC}"
echo "--------------------------------------"
echo "  A single high-volume, trusted attestor needs more headroom than its"
echo "  role grants everyone else. Address overrides take precedence over"
echo "  both the role override and the global default."
echo ""
echo "  stellar contract invoke \\"
echo "    --id $CONTRACT_ID --source $ADMIN_ACCOUNT \\"
echo "    --rpc-url \$RPC_URL --network-passphrase \"\$NETWORK_PASSPHRASE\" \\"
echo "    -- set_address_rate_limit \\"
echo "    --caller $ADMIN_ACCOUNT --address $HIGH_VOLUME_ATTESTOR \\"
echo "    --config '{\"max_submissions\":100,\"window_length\":100}'"
echo ""
echo "  Rust API:"
cat <<'RUST'
    AnchorKitContract::set_address_rate_limit(
        env, admin, high_volume_attestor.clone(),
        RateLimitConfig { max_submissions: 100, window_length: 100 },
    );
RUST
echo ""
echo -e "  ${GREEN}✓ Address override active for $HIGH_VOLUME_ATTESTOR${NC}"
echo ""

# ── Step 4: Read overrides back ──────────────────────────────────────────────
echo -e "${BLUE}Step 4: Read overrides back${NC}"
echo "--------------------------------------"
cat <<'RUST'
    let role_cfg    = AnchorKitContract::get_role_rate_limit(env.clone(), Symbol::new(&env, "KycAdmin"));
    let address_cfg = AnchorKitContract::get_address_rate_limit(env, high_volume_attestor);
    // Both return Option<RateLimitConfig>; None means "no override, uses the next
    // tier down" (role falls back to global default; address falls back to role
    // then global default).
RUST
echo ""

# ── Summary ───────────────────────────────────────────────────────────────────
echo "=== Rate Limit Resolution Order ==="
echo ""
echo "  1. Per-address override   (set_address_rate_limit / get_address_rate_limit)"
echo "  2. Per-role override      (set_role_rate_limit / get_role_rate_limit)"
echo "  3. Global default         (set_rate_limit_config / get_rate_limit_config)"
echo ""
echo "  All three setters require the contract admin or a delegate holding"
echo "  AdminCapability::SetRateLimits, and reject max_submissions == 0 or"
echo "  window_length == 0 with ErrorCode::ValidationError."
echo ""
echo "  Further reading:"
echo "    docs/governance-and-security.md"
echo "    src/rate_limiter.rs"
