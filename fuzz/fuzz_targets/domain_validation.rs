#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt to validate arbitrary bytes as domain
    if let Ok(domain_str) = std::str::from_utf8(data) {
        use anchorkit::domain_validator::DomainValidator;

        let validator = DomainValidator::new();
        // Should not panic on any input
        let _result = validator.validate(domain_str);

        // Also test with policy validation
        use anchorkit::domain_validator::DomainPolicy;
        let policy = DomainPolicy::allow_all();
        let _policy_result = policy.permits(domain_str);
    }
});
