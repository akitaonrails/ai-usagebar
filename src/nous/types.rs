//! Strict, non-secret wire models for the Nous Research OAuth/account APIs.
//!
//! The wire payloads are deliberately parsed into private-ish, display-safe
//! values.  Additive fields are ignored, but fields used by the device flow or
//! credential exchange are required and validated before callers can use them.

use std::fmt;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

const DEVICE_CODE: &str = "device_code";
const USER_CODE: &str = "user_code";
const VERIFICATION_URI: &str = "verification_uri";
const VERIFICATION_URI_COMPLETE: &str = "verification_uri_complete";
const MAX_OAUTH_FIELD_BYTES: usize = 64 * 1024;
const MAX_VERIFICATION_URL_BYTES: usize = 8 * 1024;
const PORTAL_HOST: &str = "portal.nousresearch.com";
/// A spent allocation comes back as float residue rather than a clean zero
/// (observed `-1.64e-20` on a drained Ultra plan). Anything within this
/// distance of zero is treated as zero; a genuine negative balance is still an
/// error, so the guard against nonsense payloads stays.
const CREDIT_ZERO_EPSILON: f64 = 1e-6;

/// OAuth device authorization data returned by the portal.
///
/// Device/user codes are secret-bearing during the short authorization window;
/// the custom `Debug` implementation intentionally does not print them.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCode")
            .field("device_code", &"<redacted>")
            .field("user_code", &"<redacted>")
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// A validated OAuth token response.
///
/// Tokens never appear in `Debug`; callers should also avoid formatting this
/// value directly in user-facing errors.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Display-safe subset of the Nous account response.
///
/// The account response may include internal user/organization IDs and future
/// fields.  Those values are intentionally not represented here.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSnapshot {
    pub plan: Option<String>,
    pub tier: Option<i64>,
    pub monthly_credits: Option<f64>,
    pub credits_remaining: Option<f64>,
    pub purchased_credits_remaining: Option<f64>,
    pub total_usable_credits: Option<f64>,
    pub rollover_credits: Option<f64>,
    pub current_period_end: Option<DateTime<Utc>>,
}

// Parsing rejects non-finite credit values, so equality remains reflexive.
impl Eq for AccountSnapshot {}

impl AccountSnapshot {
    /// Percentage consumed from the monthly allocation, if the response gives
    /// a complete, positive denominator and a non-negative numerator.
    pub fn usage_percent(&self) -> Option<f64> {
        let monthly = self.monthly_credits?;
        let remaining = self.credits_remaining?;
        if !monthly.is_finite() || !remaining.is_finite() || monthly <= 0.0 || remaining < 0.0 {
            return None;
        }
        Some(((monthly - remaining) / monthly * 100.0).clamp(0.0, 100.0))
    }
}

/// Parse the official device-code response, ignoring additive fields.
pub fn parse_device_code(value: &Value) -> Result<DeviceCode, String> {
    let object = object(value, "device-code response")?;
    let device_code = required_nonempty_string(object, DEVICE_CODE)?;
    let user_code = required_nonempty_string(object, USER_CODE)?;
    let verification_uri = required_https_url(object, VERIFICATION_URI)?;
    let verification_uri_complete = required_https_url(object, VERIFICATION_URI_COMPLETE)?;
    let expires_in = required_positive_u64(object, "expires_in")?;
    let interval = required_positive_u64(object, "interval")?;

    Ok(DeviceCode {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in,
        interval,
    })
}

/// Parse a successful device/refresh token response.
pub fn parse_token(value: &Value) -> Result<TokenResponse, String> {
    let object = object(value, "token response")?;
    let access_token = required_nonempty_string(object, "access_token")?;
    let refresh_token = required_nonempty_string(object, "refresh_token")?;
    let token_type = match object.get("token_type") {
        Some(_) => required_nonempty_string(object, "token_type")?,
        None => "Bearer".to_string(),
    };
    if !token_type.eq_ignore_ascii_case("bearer") {
        return Err("token response has unsupported token_type".into());
    }
    let expires_in = required_positive_u64(object, "expires_in")?;

    Ok(TokenResponse {
        access_token,
        refresh_token,
        token_type,
        expires_in,
    })
}

/// Parse an account response into a display-safe snapshot.
pub fn parse_account(value: &Value) -> Result<AccountSnapshot, String> {
    let object = object(value, "account response")?;
    if object.contains_key("error") || object.contains_key("errors") {
        return Err("account response is an error envelope".into());
    }

    let mut sources = Vec::with_capacity(3);
    if let Some(subscription) = object.get("subscription").and_then(Value::as_object) {
        sources.push(subscription);
    }
    if let Some(access) = object.get("paid_service_access").and_then(Value::as_object) {
        sources.push(access);
    }
    sources.push(object);

    let plan = optional_nonempty_string(&sources, &["plan", "plan_name", "planName"])?;
    let tier =
        optional_nonnegative_i64(&sources, &["tier", "subscription_tier", "subscriptionTier"])?;
    let monthly_credits = optional_credit(&sources, &["monthly_credits", "monthlyCredits"])?;
    let credits_remaining = optional_credit(
        &sources,
        &[
            "credits_remaining",
            "creditsRemaining",
            "subscription_credits_remaining",
            "subscriptionCreditsRemaining",
        ],
    )?;
    let purchased_credits_remaining = optional_credit(
        &sources,
        &[
            "purchased_credits_remaining",
            "purchasedCreditsRemaining",
            "top_up_credits_remaining",
            "topUpCreditsRemaining",
        ],
    )?;
    let total_usable_credits = optional_credit(
        &sources,
        ["total_usable_credits", "totalUsableCredits"].as_slice(),
    )?;
    let rollover_credits = optional_credit(
        &sources,
        &[
            "rollover_credits",
            "rolloverCredits",
            "additional_credits",
            "additionalCredits",
        ],
    )?;
    let current_period_end = optional_timestamp(
        &sources,
        &[
            "period_end",
            "current_period_end",
            "currentPeriodEnd",
            "renewal_at",
            "renewalAt",
        ],
    )?;

    // A payload containing only an internal ID or an unrelated error/status is
    // not an account contract. Nested subscription/access objects are valid
    // account envelopes even when their optional metrics are unavailable.
    let has_known_field = [
        &["plan", "plan_name", "planName"][..],
        &["tier", "subscription_tier", "subscriptionTier"][..],
        &["monthly_credits", "monthlyCredits"][..],
        &[
            "credits_remaining",
            "creditsRemaining",
            "subscription_credits_remaining",
            "subscriptionCreditsRemaining",
        ][..],
        &["total_usable_credits", "totalUsableCredits"][..],
        &[
            "purchased_credits_remaining",
            "purchasedCreditsRemaining",
            "top_up_credits_remaining",
            "topUpCreditsRemaining",
        ][..],
        &[
            "rollover_credits",
            "rolloverCredits",
            "additional_credits",
            "additionalCredits",
        ][..],
        &[
            "period_end",
            "current_period_end",
            "currentPeriodEnd",
            "renewal_at",
            "renewalAt",
        ][..],
    ]
    .into_iter()
    .flatten()
    .any(|key| sources.iter().any(|source| source.contains_key(*key)))
        || object.get("subscription").is_some_and(Value::is_object)
        || object
            .get("paid_service_access")
            .is_some_and(Value::is_object);
    if !has_known_field {
        return Err("account response has no supported display fields".into());
    }

    Ok(AccountSnapshot {
        plan,
        tier,
        monthly_credits,
        credits_remaining,
        purchased_credits_remaining,
        total_usable_credits,
        rollover_credits,
        current_period_end,
    })
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{name} must be a JSON object"))
}

fn required_nonempty_string(object: &Map<String, Value>, field: &str) -> Result<String, String> {
    let value = object
        .get(field)
        .ok_or_else(|| format!("missing required field `{field}`"))?;
    let text = value
        .as_str()
        .ok_or_else(|| format!("field `{field}` must be a string"))?;
    if text.trim().is_empty()
        || text.len() > MAX_OAUTH_FIELD_BYTES
        || text.chars().any(char::is_control)
    {
        return Err(format!("field `{field}` must be non-empty"));
    }
    Ok(text.to_owned())
}

fn required_https_url(object: &Map<String, Value>, field: &str) -> Result<String, String> {
    let value = required_nonempty_string(object, field)?;
    let parsed =
        reqwest::Url::parse(&value).map_err(|_| format!("field `{field}` must be an HTTPS URL"))?;
    if value.len() > MAX_VERIFICATION_URL_BYTES
        || parsed.scheme() != "https"
        || parsed.host_str() != Some(PORTAL_HOST)
        || parsed.port_or_known_default() != Some(443)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(format!("field `{field}` must be an HTTPS URL"));
    }
    Ok(value)
}

fn required_positive_u64(object: &Map<String, Value>, field: &str) -> Result<u64, String> {
    let value = object
        .get(field)
        .ok_or_else(|| format!("missing required field `{field}`"))?;
    let number = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
    .ok_or_else(|| format!("field `{field}` must be a positive integer"))?;
    if number == 0 {
        return Err(format!("field `{field}` must be positive"));
    }
    Ok(number)
}

fn first<'a>(objects: &[&'a Map<String, Value>], fields: &[&str]) -> Option<&'a Value> {
    objects
        .iter()
        .find_map(|object| fields.iter().find_map(|field| object.get(*field)))
}

fn optional_nonempty_string(
    objects: &[&Map<String, Value>],
    fields: &[&str],
) -> Result<Option<String>, String> {
    let Some(value) = first(objects, fields) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = value
        .as_str()
        .ok_or_else(|| "account text field must be a string".to_string())?;
    if text.trim().is_empty() || text.chars().any(char::is_control) {
        return Err("account text field must be non-empty".into());
    }
    Ok(Some(text.to_owned()))
}

fn optional_nonnegative_i64(
    objects: &[&Map<String, Value>],
    fields: &[&str],
) -> Result<Option<i64>, String> {
    let Some(value) = first(objects, fields) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_i64()
        .ok_or_else(|| "account tier must be a non-negative integer".to_string())?;
    if number < 0 {
        return Err("account tier cannot be negative".into());
    }
    Ok(Some(number))
}

fn optional_credit(
    objects: &[&Map<String, Value>],
    fields: &[&str],
) -> Result<Option<f64>, String> {
    let Some(value) = first(objects, fields) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| "account credit is not a finite number".to_string())?,
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| "account credit is not numeric".to_string())?,
        _ => return Err("account credit must be a number".into()),
    };
    if !number.is_finite() {
        return Err("account credit must be finite and non-negative".into());
    }
    if number < 0.0 {
        if number > -CREDIT_ZERO_EPSILON {
            return Ok(Some(0.0));
        }
        return Err("account credit must be finite and non-negative".into());
    }
    Ok(Some(number))
}

fn optional_timestamp(
    objects: &[&Map<String, Value>],
    fields: &[&str],
) -> Result<Option<DateTime<Utc>>, String> {
    let Some(value) = first(objects, fields) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = value
        .as_str()
        .ok_or_else(|| "account period end must be an RFC3339 string".to_string())?;
    DateTime::parse_from_rfc3339(text)
        .map(|date| date.with_timezone(&Utc))
        .map(Some)
        .map_err(|_| "account period end is not a valid RFC3339 timestamp".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_device_and_token_values() {
        let device = DeviceCode {
            device_code: "test-device-secret".into(),
            user_code: "test-user-secret".into(),
            verification_uri: "https://portal.nousresearch.com/device".into(),
            verification_uri_complete:
                "https://portal.nousresearch.com/device?code=test-user-secret".into(),
            expires_in: 900,
            interval: 5,
        };
        let token = TokenResponse {
            access_token: "test-access-secret".into(),
            refresh_token: "test-refresh-secret".into(),
            token_type: "Bearer".into(),
            expires_in: 3600,
        };
        let output = format!("{device:?} {token:?}");
        assert!(!output.contains("test-device-secret"));
        assert!(!output.contains("test-user-secret"));
        assert!(!output.contains("test-access-secret"));
        assert!(!output.contains("test-refresh-secret"));
    }

    #[test]
    fn account_snapshot_only_retains_display_safe_fields() {
        let value = serde_json::json!({
            "plan": "Pro",
            "user_id": "test-user-id",
            "organization_id": "test-org-id",
            "monthly_credits": 10.0,
        });
        let snapshot = parse_account(&value).unwrap();
        let debug = format!("{snapshot:?}");
        assert!(debug.contains("Pro"));
        assert!(!debug.contains("test-user-id"));
        assert!(!debug.contains("test-org-id"));
    }

    #[test]
    fn oauth_urls_are_structural_and_secret_fields_are_bounded() {
        let mut device = serde_json::json!({
            "device_code": "test-device",
            "user_code": "TEST",
            "verification_uri": "https://portal.nousresearch.com/device",
            "verification_uri_complete": "https://portal.nousresearch.com/device?code=TEST",
            "expires_in": 900,
            "interval": 5
        });
        assert!(parse_device_code(&device).is_ok());

        device["verification_uri_complete"] =
            serde_json::json!("https://user@portal.nousresearch.com/device");
        assert!(parse_device_code(&device).is_err());
        device["verification_uri_complete"] =
            serde_json::json!("https://portal.nousresearch.com.evil.test/device");
        assert!(parse_device_code(&device).is_err());

        let token = serde_json::json!({
            "access_token": "x".repeat(MAX_OAUTH_FIELD_BYTES + 1),
            "refresh_token": "test-refresh",
            "expires_in": 3600
        });
        assert!(parse_token(&token).is_err());
    }
}
