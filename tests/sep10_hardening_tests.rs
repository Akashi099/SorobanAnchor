#![cfg(test)]

mod sep10_test_util;

mod sep10_hardening_tests {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Bytes, Env, String};

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
    use anchorkit::sep10_jwt::{verify_sep10_jwt, verify_sep10_jwt_with_issuer, MAX_JWT_LIFETIME};
    use crate::sep10_test_util::{
        build_sep10_jwt, build_sep10_jwt_with_iat, build_sep10_jwt_with_iss,
        build_sep10_jwt_with_future_iat, build_sep10_jwt_whitespace_iss,
        build_sep10_jwt_empty_sub,
    };

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn ledger(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
    }

    fn make_contract_id(env: &Env) -> Address {
        env.register_contract(None, AnchorKitContract)
    }

    fn setup(env: &Env, ts: u64) -> (AnchorKitContractClient, Address, SigningKey) {
        ledger(env, ts);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize(&admin);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(env, sk.verifying_key().as_bytes());
        let issuer = Address::generate(env);
        client.set_sep10_jwt_verifying_key(&issuer, &pk);
        (client, issuer, sk)
    }

    // -----------------------------------------------------------------------
    // Issue #766: empty and whitespace-only tokens are rejected up front,
    // before any base64/JSON decoding is attempted.
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_empty_token() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());

        let token = String::from_str(&env, "");

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "empty token must be rejected"
            );
        });
    }

    #[test]
    fn rejects_whitespace_only_token() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());

        let token = String::from_str(&env, "   ");

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "whitespace-only token must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // Issue #767: a token must have exactly three dot-separated parts.
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_four_part_token() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // An otherwise well-formed token with one extra trailing segment.
        let jwt = build_sep10_jwt(&sk, &sub_str, 5_000);
        let four_part_jwt = format!("{}.extra", jwt);
        let token = String::from_str(&env, &four_part_jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "four-part token must be rejected"
            );
        });
    }

    #[test]
    fn accepts_structurally_valid_three_part_token() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt(&sk, &sub_str, 5_000);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_ok(),
                "structurally valid three-part token must reach verification"
            );
        });
    }

    // -----------------------------------------------------------------------
    // iat: must be present (already tested upstream — confirm still rejected)
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_missing_iat() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // Build token without iat
        let header = r#"{"alg":"EdDSA","typ":"JWT"}"#;
        let payload = format!(
            r#"{{"sub":"{}","exp":{},"iss":"https://anchor.example.com"}}"#,
            sub_str, 5_000u64
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let sig = sk.sign(signing_input.as_bytes());
        let jwt = format!("{}.{}", signing_input, URL_SAFE_NO_PAD.encode(sig.to_bytes()));
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(verify_sep10_jwt(&env, &token, &pk, None).is_err());
        });
    }

    // -----------------------------------------------------------------------
    // iat: must be zero — explicit zero iat rejected
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_with_zero_iat() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt_with_iat(&sk, &sub_str, 0, 5_000);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "zero iat must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // iat: must not be in the future
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_with_future_iat() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        let now = 1_000u64;
        ledger(&env, now);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // iat = now + 200, well beyond the 60 s skew tolerance
        let jwt = build_sep10_jwt_with_future_iat(&sk, &sub_str, now, 200);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "future iat must be rejected"
            );
        });
    }

    #[test]
    fn accepts_token_with_iat_within_skew_window() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        let now = 1_000u64;
        ledger(&env, now);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // iat = now + 30 (within default 60 s skew)
        let iat = now + 30;
        let exp = iat + MAX_JWT_LIFETIME;
        let jwt = build_sep10_jwt_with_iat(&sk, &sub_str, iat, exp);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_ok(),
                "iat within skew should be accepted"
            );
        });
    }

    // -----------------------------------------------------------------------
    // sub: must be non-empty
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_with_empty_sub() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());

        let jwt = build_sep10_jwt_empty_sub(&sk, 5_000);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "empty sub must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // iss: whitespace-only must be rejected
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_with_whitespace_only_iss() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt_whitespace_iss(&sk, &sub_str, 5_000);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "whitespace-only iss must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // iss: control character in issuer is rejected
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_token_with_control_char_in_iss() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // iss containing a null byte (0x00)
        let jwt = build_sep10_jwt_with_iss(&sk, &sub_str, 5_000, "https://evil\x00.com");
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt(&env, &token, &pk, None).is_err(),
                "control char in iss must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // verify_sep10_jwt_with_issuer: happy path
    // -----------------------------------------------------------------------

    #[test]
    fn with_issuer_accepts_matching_iss() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt_with_iss(&sk, &sub_str, 5_000, "https://anchor.example.com");
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt_with_issuer(&env, &token, &pk, None, "https://anchor.example.com").is_ok()
            );
        });
    }

    // -----------------------------------------------------------------------
    // verify_sep10_jwt_with_issuer: issuer mismatch is rejected
    // -----------------------------------------------------------------------

    #[test]
    fn with_issuer_rejects_mismatched_iss() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt_with_iss(&sk, &sub_str, 5_000, "https://anchor.example.com");
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt_with_issuer(&env, &token, &pk, None, "https://evil.example.com").is_err(),
                "mismatched issuer must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // verify_sep10_jwt_with_issuer: empty expected_issuer is rejected
    // -----------------------------------------------------------------------

    #[test]
    fn with_issuer_rejects_empty_expected_issuer() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        let jwt = build_sep10_jwt(&sk, &sub_str, 5_000);
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            assert!(
                verify_sep10_jwt_with_issuer(&env, &token, &pk, None, "").is_err(),
                "empty expected_issuer must be rejected"
            );
        });
    }

    // -----------------------------------------------------------------------
    // verify_sep10_jwt_with_issuer: whitespace trimming on both sides
    // -----------------------------------------------------------------------

    #[test]
    fn with_issuer_trims_whitespace_before_comparison() {
        let env = make_env();
        let contract_id = make_contract_id(&env);
        ledger(&env, 1_000);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = Bytes::from_slice(&env, sk.verifying_key().as_bytes());
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();

        // iss in token has surrounding spaces
        let jwt = build_sep10_jwt_with_iss(&sk, &sub_str, 5_000, "  https://anchor.example.com  ");
        let token = String::from_str(&env, &jwt);

        env.as_contract(&contract_id, || {
            // Should match after trimming
            assert!(
                verify_sep10_jwt_with_issuer(
                    &env, &token, &pk, None,
                    "https://anchor.example.com"
                ).is_ok(),
                "trimmed iss should match trimmed expected_issuer"
            );
        });
    }

    // -----------------------------------------------------------------------
    // Contract-level: expired token is clearly rejected (regression)
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn contract_rejects_expired_token() {
        let env = make_env();
        let (client, issuer, sk) = setup(&env, 10_000);
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();
        // Token expired 200 s ago (beyond 60 s skew)
        let jwt = build_sep10_jwt(&sk, &sub_str, 9_000);
        let token = String::from_str(&env, &jwt);
        client.verify_sep10_token(&token, &issuer);
    }

    // -----------------------------------------------------------------------
    // Contract-level: malformed token (wrong number of dots) is rejected
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn contract_rejects_malformed_token() {
        let env = make_env();
        let (client, issuer, _) = setup(&env, 1_000);
        let token = String::from_str(&env, "not.a.valid.jwt.token");
        client.verify_sep10_token(&token, &issuer);
    }

    // -----------------------------------------------------------------------
    // Contract-level: replayed token is rejected (cross-ledger JTI cache)
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn contract_rejects_replayed_token() {
        use crate::sep10_test_util::build_sep10_jwt_with_jti;
        let env = make_env();
        let (client, issuer, sk) = setup(&env, 1_000);
        let sub = Address::generate(&env).to_string();
        let sub_str: std::string::String = sub.to_string();
        let exp = 1_000u64 + 3_600;
        let jwt = build_sep10_jwt_with_jti(&sk, &sub_str, exp, "replay-hardening-jti");
        let token = String::from_str(&env, &jwt);
        client.verify_sep10_token(&token, &issuer); // first — OK
        client.verify_sep10_token(&token, &issuer); // second — must panic
    }
}
