//! Nous Research OAuth and subscription usage integration.
//!
//! Credential storage and OAuth secrets stay inside this module. Shared UI
//! integration receives only the display-safe [`AccountSnapshot`].

pub mod credentials;
pub mod fetch;
pub mod oauth;
pub mod types;
pub mod vendor;

pub use types::{
    AccountSnapshot, DeviceCode, TokenResponse, parse_account, parse_device_code, parse_token,
};

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::types::{parse_account, parse_device_code, parse_token};

    fn fixture(path: &str) -> Value {
        serde_json::from_str(path).expect("fixture JSON must be valid")
    }

    #[test]
    fn official_device_fixture_uses_expected_fields() {
        let value = fixture(include_str!("../../tests/fixtures/nous/device-code.json"));
        let parsed = parse_device_code(&value).expect("official device fixture must parse");
        assert_eq!(parsed.device_code, "test-device-code");
        assert_eq!(parsed.user_code, "TEST-USER");
        assert_eq!(
            parsed.verification_uri,
            "https://portal.nousresearch.com/device"
        );
        assert_eq!(parsed.expires_in, 900);
        assert_eq!(parsed.interval, 5);
    }

    #[test]
    fn official_token_fixture_keeps_token_fields_separate() {
        let value = fixture(include_str!("../../tests/fixtures/nous/token-success.json"));
        let parsed = parse_token(&value).expect("official token fixture must parse");
        assert_eq!(parsed.access_token, "test-access-token");
        assert_eq!(parsed.refresh_token, "test-refresh-token");
        assert_eq!(parsed.expires_in, 3600);
    }

    #[test]
    fn portal_token_response_accepts_string_ttl_and_omitted_token_type() {
        let mut value = fixture(include_str!("../../tests/fixtures/nous/token-success.json"));
        value.as_object_mut().unwrap().remove("token_type");
        value["expires_in"] = serde_json::json!("3600");
        let parsed = parse_token(&value).expect("portal token response must parse");
        assert_eq!(parsed.token_type, "Bearer");
        assert_eq!(parsed.expires_in, 3600);
    }

    #[test]
    fn portal_device_response_accepts_string_ttl_and_interval() {
        let mut value = fixture(include_str!("../../tests/fixtures/nous/device-code.json"));
        value["expires_in"] = serde_json::json!("900");
        value["interval"] = serde_json::json!("5");
        let parsed = parse_device_code(&value).expect("portal device response must parse");
        assert_eq!(parsed.expires_in, 900);
        assert_eq!(parsed.interval, 5);
    }

    #[test]
    fn official_account_fixture_keeps_internal_identifiers_out_of_snapshot() {
        let value = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        let parsed = parse_account(&value).expect("official account fixture must parse");
        assert_eq!(parsed.plan.as_deref(), Some("Pro"));
        assert_eq!(parsed.monthly_credits, Some(1000.0));
        assert_eq!(parsed.credits_remaining, Some(760.0));
        assert_eq!(parsed.rollover_credits, Some(40.0));
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("internal-user"));
        assert!(!debug.contains("internal-org"));
    }

    #[test]
    fn nested_portal_account_payload_is_normalized_for_display() {
        let value = serde_json::json!({
            "user": {"email": "hidden@example.test"},
            "organisation": {"id": "internal-org"},
            "subscription": {
                "plan": "Plus",
                "monthly_credits": "22",
                "credits_remaining": "17.5",
                "rollover_credits": 2.0,
                "current_period_end": "2026-09-01T00:00:00Z"
            },
            "paid_service_access": {
                "allowed": true,
                "purchased_credits_remaining": "5.25",
                "total_usable_credits": "22.75"
            }
        });
        let parsed = parse_account(&value).expect("nested Portal account must parse");
        assert_eq!(parsed.plan.as_deref(), Some("Plus"));
        assert_eq!(parsed.monthly_credits, Some(22.0));
        assert_eq!(parsed.credits_remaining, Some(17.5));
        assert_eq!(parsed.purchased_credits_remaining, Some(5.25));
        assert_eq!(parsed.total_usable_credits, Some(22.75));
        assert_eq!(parsed.rollover_credits, Some(2.0));
        assert_eq!(
            parsed.current_period_end.unwrap().to_rfc3339(),
            "2026-09-01T00:00:00+00:00"
        );
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("hidden@example.test"));
        assert!(!debug.contains("internal-org"));
    }

    #[test]
    fn subscription_percentage_never_uses_top_up_or_total_usable_credits() {
        let value = serde_json::json!({
            "subscription": {
                "plan": "Plus",
                "monthly_credits": 20.0,
                "current_period_end": "2026-09-01T00:00:00Z"
            },
            "paid_service_access": {
                "subscription_credits_remaining": 8.0,
                "purchased_credits_remaining": 12.0,
                "total_usable_credits": 20.0
            }
        });
        let parsed = parse_account(&value).expect("official nested balances must parse");
        assert_eq!(parsed.credits_remaining, Some(8.0));
        assert_eq!(parsed.purchased_credits_remaining, Some(12.0));
        assert_eq!(parsed.total_usable_credits, Some(20.0));
        assert_eq!(parsed.usage_percent(), Some(60.0));

        let total_only = parse_account(&serde_json::json!({
            "subscription": {"monthly_credits": 20.0},
            "paid_service_access": {"total_usable_credits": 20.0}
        }))
        .expect("total usable remains a displayable balance");
        assert_eq!(total_only.credits_remaining, None);
        assert_eq!(total_only.usage_percent(), None);
    }

    #[test]
    fn device_code_requires_the_complete_verification_uri() {
        let mut value = fixture(include_str!("../../tests/fixtures/nous/device-code.json"));
        value
            .as_object_mut()
            .unwrap()
            .remove("verification_uri_complete");
        assert!(parse_device_code(&value).is_err());
    }

    #[test]
    fn additive_device_code_fields_are_ignored() {
        let mut value = fixture(include_str!("../../tests/fixtures/nous/device-code.json"));
        value["future_field"] = serde_json::json!({"ignored": true});
        assert!(parse_device_code(&value).is_ok());
    }

    #[test]
    fn empty_or_expired_token_credentials_are_rejected() {
        for (field, value) in [("access_token", ""), ("refresh_token", "")] {
            let mut payload = fixture(include_str!("../../tests/fixtures/nous/token-success.json"));
            payload[field] = serde_json::json!(value);
            assert!(parse_token(&payload).is_err(), "{field} must be non-empty");
        }
        let mut payload = fixture(include_str!("../../tests/fixtures/nous/token-success.json"));
        payload["expires_in"] = serde_json::json!(0);
        assert!(parse_token(&payload).is_err());
    }

    #[test]
    fn invalid_account_credits_and_period_are_rejected() {
        let mut negative = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        negative["credits_remaining"] = serde_json::json!(-1.0);
        assert!(parse_account(&negative).is_err());

        let mut non_finite = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        non_finite["monthly_credits"] = serde_json::json!("NaN");
        assert!(parse_account(&non_finite).is_err());

        let mut bad_period = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        bad_period["period_end"] = serde_json::json!("not-a-timestamp");
        assert!(parse_account(&bad_period).is_err());
    }

    #[test]
    fn spent_allocation_residue_reads_as_zero() {
        // A drained plan reports the rounding residue instead of a clean zero;
        // that must still render, while a real negative balance stays an error.
        let mut residue = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        residue["credits_remaining"] = serde_json::json!(-1.64e-20);
        let parsed = parse_account(&residue).expect("residue is not a negative balance");
        assert_eq!(parsed.credits_remaining, Some(0.0));
        assert_eq!(parsed.usage_percent(), Some(100.0));
    }

    #[test]
    fn absent_optional_account_metrics_stay_unavailable() {
        let value = serde_json::json!({"plan": "Free", "future": "allowed"});
        let parsed = parse_account(&value).expect("optional metrics may be absent");
        assert_eq!(parsed.plan.as_deref(), Some("Free"));
        assert_eq!(parsed.monthly_credits, None);
        assert_eq!(parsed.credits_remaining, None);
        assert_eq!(parsed.rollover_credits, None);
    }
}
