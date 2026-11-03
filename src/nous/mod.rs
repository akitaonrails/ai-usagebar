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
    AccountSnapshot, DeviceCode, TokenResponse, UsageGauge, parse_account, parse_device_code,
    parse_token,
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
        assert_eq!(parsed.plan.as_deref(), Some("Ultra"));
        assert_eq!(parsed.monthly_credits, Some(0.0));
        assert_eq!(parsed.credits_remaining, Some(0.0));
        assert_eq!(parsed.rollover_credits, Some(0.0));
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("internal-user"));
        assert!(!debug.contains("internal-org"));
    }

    #[test]
    fn usage_window_gauges_are_parsed_with_labels_and_resets() {
        let value = fixture(include_str!("../../tests/fixtures/nous/account.json"));
        let parsed = parse_account(&value).expect("usage_window fixture must parse");
        let ids: Vec<_> = parsed.gauges.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["day", "week", "month", "top_tier"]);
        assert_eq!(parsed.gauges[0].percent(), 1.8);
        assert_eq!(parsed.gauges[1].percent(), 31.31);
        assert_eq!(parsed.gauges[2].percent(), 10.43);
        assert_eq!(parsed.gauges[3].percent(), 0.3);
        assert_eq!(parsed.gauges[3].scope.as_deref(), Some("top_tier"));
        assert_eq!(parsed.gauges[2].label.as_deref(), Some("This month"));
        assert_eq!(
            parsed.gauge("day").unwrap().reset_at.unwrap().to_rfc3339(),
            "2026-11-04T00:00:00+00:00"
        );
        // The week is the most-exhausted reported window, so it is the
        // headline — the month is broader but nowhere near its limit.
        assert_eq!(parsed.primary_gauge().unwrap().id, "week");
        assert_eq!(parsed.usage_percent(), Some(31.31));
    }

    #[test]
    fn flat_basis_point_fields_fill_in_when_gauges_are_absent() {
        let parsed = parse_account(&serde_json::json!({
            "plan": "Ultra",
            "monthly_credits": 0.0,
            "credits_remaining": 0.0,
            "day_used_bp": 180,
            "week_used_bp": "3131",
            "month_used_bp": 1043,
            "top_tier_used_bp": 30,
            "day_resets_at_ms": 1793750400000i64,
            "month_resets_at_ms": "1796083200000"
        }))
        .expect("flat usage fields must parse");
        let ids: Vec<_> = parsed.gauges.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["day", "week", "month", "top_tier"]);
        assert_eq!(parsed.gauges[1].used_bp, 3131.0);
        assert_eq!(
            parsed
                .gauge("month")
                .unwrap()
                .reset_at
                .unwrap()
                .to_rfc3339(),
            "2026-12-01T00:00:00+00:00"
        );
        assert_eq!(parsed.usage_percent(), Some(31.31));
    }

    #[test]
    fn the_headline_window_is_the_one_closest_to_its_limit() {
        // A plan can be blocked today and idle for the month. `headline_pct`,
        // the Overview bar and the Waybar severity all read this figure, so it
        // has to be the window that is actually spent.
        let parsed = parse_account(&serde_json::json!({
            "plan": "Ultra",
            "usage_window": {"gauges": [
                {"id": "day", "used_bp": 9900},
                {"id": "week", "used_bp": 4000},
                {"id": "month", "used_bp": 1043}
            ]}
        }))
        .expect("account must parse");
        assert_eq!(parsed.primary_gauge().unwrap().id, "day");
        assert_eq!(parsed.usage_percent(), Some(99.0));

        // Equally spent windows settle on the broadest, so the headline does
        // not flip between two identical figures between refreshes.
        let tied = parse_account(&serde_json::json!({
            "plan": "Ultra",
            "usage_window": {"gauges": [
                {"id": "day", "used_bp": 4000},
                {"id": "week", "used_bp": 4000},
                {"id": "month", "used_bp": 4000}
            ]}
        }))
        .expect("account must parse");
        assert_eq!(tied.primary_gauge().unwrap().id, "month");

        // An unknown window still renders as its own row, but it cannot be the
        // headline: we do not know what it measures.
        let unknown = parse_account(&serde_json::json!({
            "plan": "Ultra",
            "usage_window": {"gauges": [
                {"id": "future_window", "used_bp": 9900},
                {"id": "month", "used_bp": 1043}
            ]}
        }))
        .expect("account must parse");
        assert!(unknown.has_usage_windows());
        assert_eq!(unknown.primary_gauge().unwrap().id, "month");
    }

    #[test]
    fn absent_usage_window_keeps_the_credit_percentage() {
        // Pre-migration accounts: no windows, so the credit allocation is
        // still the consumption figure. This is the identical old behaviour.
        let parsed = parse_account(&serde_json::json!({
            "plan": "Pro",
            "monthly_credits": 1000.0,
            "credits_remaining": 760.0
        }))
        .expect("credits-only account must parse");
        assert!(parsed.gauges.is_empty());
        assert!(!parsed.has_usage_windows());
        assert_eq!(parsed.primary_gauge(), None);
        assert_eq!(parsed.usage_percent(), Some(24.0));
    }

    #[test]
    fn invalid_basis_points_are_rejected_and_unreported_windows_are_omitted() {
        for invalid in [serde_json::json!(-1), serde_json::json!("NaN")] {
            let value = serde_json::json!({
                "plan": "Ultra",
                "usage_window": {"gauges": [{"id": "day", "used_bp": invalid}]}
            });
            assert!(parse_account(&value).is_err(), "{invalid} must be rejected");
        }

        // A window the portal did not measure is a missing metric, never a
        // spent one: no `used_bp` means the gauge is dropped.
        let value = serde_json::json!({
            "plan": "Ultra",
            "usage_window": {"gauges": [
                {"id": "day", "used_bp": 180},
                {"id": "week", "label": "This week"},
                {"id": "future_window", "used_bp": 2500}
            ]}
        });
        let parsed = parse_account(&value).expect("partial gauges must parse");
        let ids: Vec<_> = parsed.gauges.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["day", "future_window"]);
        // An unknown id is retained but never becomes the headline window.
        assert_eq!(parsed.primary_gauge().unwrap().id, "day");
        assert_eq!(parsed.usage_percent(), Some(1.8));
    }

    #[test]
    fn resets_at_ms_must_be_milliseconds_and_bad_ones_just_drop_the_countdown() {
        let parsed = parse_account(&serde_json::json!({
            "plan": "Ultra",
            "usage_window": {"gauges": [
                {"id": "day", "used_bp": 180, "resets_at_ms": 1793750400000i64},
                {"id": "week", "used_bp": 3131, "resets_at_ms": "not-a-number"}
            ]}
        }))
        .expect("malformed reset must not sink the metric");
        assert_eq!(
            parsed.gauge("day").unwrap().reset_at.unwrap().to_rfc3339(),
            "2026-11-04T00:00:00+00:00"
        );
        assert_eq!(parsed.gauge("week").unwrap().reset_at, None);
        assert_eq!(parsed.gauge("week").unwrap().percent(), 31.31);
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
        let residue = serde_json::json!({
            "plan": "Pro",
            "monthly_credits": 1000.0,
            "credits_remaining": -1.64e-20
        });
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
