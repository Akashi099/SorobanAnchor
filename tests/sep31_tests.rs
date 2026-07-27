//! Tests for SEP-31 direct payment support (#558).

use anchorkit::contract::{
    AnchorKitContract, AnchorKitContractClient, ServiceType, SERVICE_SEP31,
};
use anchorkit::sep31::{initiate_sep31_payment, RawSep31PaymentResponse};
use anchorkit::Error;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

use sep10_test_util::register_attestor_with_sep10;

const VALID_ACCOUNT: &str = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

fn raw_payment() -> RawSep31PaymentResponse {
    RawSep31PaymentResponse {
        id: "pay-001".into(),
        stellar_account_id: VALID_ACCOUNT.into(),
        stellar_memo: None,
        stellar_memo_type: None,
        amount: None,
        asset_code: None,
        idempotency_key: None,
    }
}

#[test]
fn valid_payment_response_accepted() {
    let resp = initiate_sep31_payment(raw_payment()).unwrap();
    assert_eq!(resp.id, "pay-001");
    assert_eq!(resp.stellar_account_id, VALID_ACCOUNT);
}

#[test]
fn empty_id_rejected() {
    let mut raw = raw_payment();
    raw.id.clear();
    assert_eq!(
        initiate_sep31_payment(raw),
        Err(Error::invalid_transaction_intent())
    );
}

#[test]
fn invalid_stellar_account_id_rejected() {
    let mut raw = raw_payment();
    raw.stellar_account_id = "invalid-account".into();
    assert!(initiate_sep31_payment(raw).is_err());
}

#[test]
fn memo_without_memo_type_rejected() {
    let mut raw = raw_payment();
    raw.stellar_memo = Some("12345".into());
    raw.stellar_memo_type = None;
    assert_eq!(
        initiate_sep31_payment(raw),
        Err(Error::invalid_transaction_intent())
    );
}

#[test]
fn memo_with_invalid_type_rejected() {
    let mut raw = raw_payment();
    raw.stellar_memo = Some("12345".into());
    raw.stellar_memo_type = Some("fax".into());
    assert_eq!(
        initiate_sep31_payment(raw),
        Err(Error::invalid_transaction_intent())
    );
}

#[test]
fn service_type_sep31_detected_in_capability_check() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, AnchorKitContract);
    let client = AnchorKitContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let anchor = Address::generate(&env);
    let sk = SigningKey::generate(&mut OsRng);
    register_attestor_with_sep10(&env, &client, &anchor, &anchor, &sk);

    let mut services = Vec::new(&env);
    services.push_back(ServiceType::Sep31.as_u32());

    client.configure_services(&anchor, &services);

    assert_eq!(ServiceType::Sep31.as_u32(), SERVICE_SEP31);
    assert!(client.supports_service(&anchor, &SERVICE_SEP31));
}
