//! Wire types for the Alibaba Cloud Model Studio Token Plan console gateway,
//! reconstructed from the official `bl` CLI (`packages/core/src/console/gateway.ts`).
//!
//! Three shape facts drive everything here:
//!
//! - The console gateway is a generic dispatcher: the real API name rides in
//!   the `api=` query param (slash-encoded) and again inside the `params`
//!   form field, beside a fixed `cornerstoneParam` console context.
//! - The response wraps the payload in a **double "DataV2" envelope** with
//!   several tolerated depths; the CLI's unwrap order is reproduced exactly
//!   in [`unwrap_payload`].
//! - `per5HourPercentage` / `per1WeekPercentage` are **ratios in [0, 1]**,
//!   not percents — `0.4217` means 42%. Reset times are epoch **milliseconds**.

use chrono::DateTime;
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::usage::{ModelStudioSnapshot, UsageWindow};

/// The billing console the credential belongs to. Unknown wire values fall
/// back to `Domestic`, the CLI's own default row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleSite {
    Domestic,
    International,
}

impl ConsoleSite {
    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("international") {
            ConsoleSite::International
        } else {
            ConsoleSite::Domestic
        }
    }
}

/// The console region. Unknown wire values fall back to `CnBeijing`, the
/// CLI's own default row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleRegion {
    CnBeijing,
    ApSoutheast1,
}

impl ConsoleRegion {
    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("ap-southeast-1") {
            ConsoleRegion::ApSoutheast1
        } else {
            ConsoleRegion::CnBeijing
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ConsoleRegion::CnBeijing => "cn-beijing",
            ConsoleRegion::ApSoutheast1 => "ap-southeast-1",
        }
    }
}

/// Host + gateway action for one region×site cell, from the CLI's gateway
/// table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gateway {
    pub host: &'static str,
    pub action: &'static str,
}

pub fn gateway_for(region: ConsoleRegion, site: ConsoleSite) -> Gateway {
    match (region, site) {
        (ConsoleRegion::CnBeijing, ConsoleSite::Domestic) => Gateway {
            host: "bailian-cs.console.aliyun.com",
            action: "BroadScopeAspnGateway",
        },
        (ConsoleRegion::CnBeijing, ConsoleSite::International) => Gateway {
            host: "bailian-cs.console.alibabacloud.com",
            action: "BroadScopeAspnGateway",
        },
        (ConsoleRegion::ApSoutheast1, ConsoleSite::Domestic) => Gateway {
            host: "modelstudio-cs.console.aliyun.com",
            action: "IntlBroadScopeAspnGateway",
        },
        (ConsoleRegion::ApSoutheast1, ConsoleSite::International) => Gateway {
            host: "bailian-singapore-cs.alibabacloud.com",
            action: "IntlBroadScopeAspnGateway",
        },
    }
}

/// The Token Plan usage API, verbatim. Every `/` is percent-encoded when it
/// rides in the `api=` query param (`encodeURIComponent` semantics).
pub const USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
/// `USAGE_API` with each `/` as `%2F` — exactly what `encodeURIComponent`
/// produces, and the only spelling the gateway accepts.
pub const USAGE_API_ENCODED: &str =
    "zeldaHttp.apikeyMgr.%2Ftokenplan%2Fpersonal%2Fapi%2Fv2%2Fusage";
/// Fixed gateway product marker.
pub const PRODUCT: &str = "sfm_bailian";

/// `/cli/api.json?action={action}&product=sfm_bailian&api={api encoded}`.
pub fn usage_path(action: &str) -> String {
    format!("/cli/api.json?action={action}&product={PRODUCT}&api={USAGE_API_ENCODED}")
}

/// The `params` form field: the API name again, the version, and the fixed
/// console context the CLI sends. Identical for every region×site cell — the
/// region itself travels in the sibling `region` field.
pub fn params_body() -> String {
    serde_json::json!({
        "Api": USAGE_API,
        "V": "1.0",
        "Data": {
            "cornerstoneParam": {
                "protocol": "V2",
                "console": "ONE_CONSOLE",
                "productCode": "p_efm",
                "switchUserType": 3,
                "consoleSite": "BAILIAN_ALIYUN",
            }
        },
    })
    .to_string()
}

/// Percent-encode a form value with `encodeURIComponent` semantics: every
/// byte outside `A-Za-z0-9-._~` becomes `%XX`. (JS also leaves `!'()*` bare;
/// no value here contains any of those.)
fn encode_form_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The two-field form body: `region={region}&params={json}`.
pub fn form_body(region: ConsoleRegion) -> String {
    format!(
        "region={}&params={}",
        encode_form_value(region.as_str()),
        encode_form_value(&params_body())
    )
}

/// The usage fields the Token Plan response carries. All optional: a missing
/// percentage means the account has no such window (possibly unlimited), and
/// the renderer must show the window as absent — never as 0%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageFields {
    pub per5_hour_percentage: Option<f64>,
    pub per1_week_percentage: Option<f64>,
    pub per5_hour_reset_ms: Option<i64>,
    pub per1_week_reset_ms: Option<i64>,
}

/// The CLI's tolerant unwrap, verbatim:
/// `data.DataV2?.data?.data ?? data.DataV2?.data ?? data.DataV2 ?? data.data ?? data`.
/// JSON `null` counts as absent, like JS `null`/`undefined` under `??`.
pub fn unwrap_payload(root: &Value) -> &Value {
    fn present(v: Option<&Value>) -> Option<&Value> {
        v.filter(|v| !v.is_null())
    }
    let datav2 = root.get("DataV2");
    present(
        datav2
            .and_then(|d| d.get("data"))
            .and_then(|d| d.get("data")),
    )
    .or_else(|| present(datav2.and_then(|d| d.get("data"))))
    .or_else(|| present(datav2))
    .or_else(|| present(root.get("data")))
    .unwrap_or(root)
}

/// Parse a full gateway response into usage fields. Failures are reported the
/// way the CLI sees them: `success === false` plus an `errorCode`, where any
/// `NotLogined`-shaped code means the console session died.
pub fn parse_response(bytes: &[u8]) -> Result<UsageFields> {
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|e| AppError::Schema(format!("modelstudio usage response: {e}")))?;
    if root.get("success").and_then(Value::as_bool) == Some(false) {
        let code = root
            .get("errorCode")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if code.contains("NotLogined") {
            return Err(reauth_error());
        }
        return Err(AppError::Schema(format!(
            "modelstudio gateway error: {code}"
        )));
    }
    let payload = unwrap_payload(&root);
    Ok(UsageFields {
        per5_hour_percentage: ratio(payload, "per5HourPercentage")?,
        per1_week_percentage: ratio(payload, "per1WeekPercentage")?,
        per5_hour_reset_ms: epoch_ms(payload, "per5HourResetTime")?,
        per1_week_reset_ms: epoch_ms(payload, "per1WeekResetTime")?,
    })
}

/// The re-auth error every dead-console-session path funnels into — the CLI's
/// own fix, naming its own command.
pub fn reauth_error() -> AppError {
    AppError::Credentials(
        "Model Studio: console session expired; run `bl auth login --console` to re-auth".into(),
    )
}

/// One optional ratio in [0, 1]. Present-but-invalid (NaN, negative, > 1) is
/// schema drift — the wire contract changed — never a silent 0.
fn ratio(payload: &Value, field: &str) -> Result<Option<f64>> {
    match payload.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let raw = v.as_f64().ok_or_else(|| drift(field, "is not a number"))?;
            if !raw.is_finite() {
                return Err(drift(field, "is not a finite number"));
            }
            if raw < 0.0 || raw > 1.0 {
                return Err(drift(field, "is outside [0,1]"));
            }
            Ok(Some(raw))
        }
    }
}

/// One optional epoch-milliseconds timestamp. Present-but-invalid is drift.
fn epoch_ms(payload: &Value, field: &str) -> Result<Option<i64>> {
    match payload.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let raw = v
                .as_i64()
                .ok_or_else(|| drift(field, "is not an integer"))?;
            if raw < 0 {
                return Err(drift(field, "is negative"));
            }
            if chrono::DateTime::from_timestamp_millis(raw).is_none() {
                return Err(drift(field, "is out of range"));
            }
            Ok(Some(raw))
        }
    }
}

fn drift(field: &str, why: &str) -> AppError {
    AppError::Schema(format!("modelstudio `{field}` {why}"))
}

/// The 5h rolling window's length — advertised by the field name, like Kimi's.
pub const FIVE_HOUR_WINDOW: chrono::Duration = chrono::Duration::hours(5);
/// The weekly window's length.
pub const WEEKLY_WINDOW: chrono::Duration = chrono::Duration::days(7);

impl UsageFields {
    pub fn to_snapshot(&self) -> Result<ModelStudioSnapshot> {
        let window =
            |ratio: Option<f64>, reset_ms: Option<i64>, duration| -> Result<Option<UsageWindow>> {
                ratio
                    .map(|r| {
                        Ok(UsageWindow {
                            utilization_pct: ratio_to_percent(r)?,
                            resets_at: match reset_ms {
                                Some(ms) => Some(
                                    DateTime::from_timestamp_millis(ms)
                                        .ok_or_else(|| drift("reset time", "is out of range"))?,
                                ),
                                None => None,
                            },
                            window_duration: duration,
                        })
                    })
                    .transpose()
            };
        Ok(ModelStudioSnapshot {
            session: window(
                self.per5_hour_percentage,
                self.per5_hour_reset_ms,
                FIVE_HOUR_WINDOW,
            )?,
            weekly: window(
                self.per1_week_percentage,
                self.per1_week_reset_ms,
                WEEKLY_WINDOW,
            )?,
        })
    }
}

/// ×100, rounded: `0.4217` → 42. The parse gate already rejected NaN and
/// out-of-range values, so this cannot invent a figure.
fn ratio_to_percent(ratio: f64) -> Result<i32> {
    let pct = (ratio * 100.0).round();
    if !(0.0..=100.0).contains(&pct) {
        return Err(AppError::Schema(format!(
            "modelstudio ratio {ratio} did not project onto 0..=100"
        )));
    }
    Ok(pct as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_envelope() -> String {
        serde_json::json!({
            "success": true,
            "DataV2": {
                "success": true,
                "data": {
                    "data": {
                        "per5HourPercentage": 0.4217,
                        "per5HourResetTime": 1789200000000_i64,
                        "per1WeekPercentage": 0.7356,
                        "per1WeekResetTime": 1789600000000_i64,
                    }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn gateway_matrix_covers_every_region_site_cell() {
        assert_eq!(
            gateway_for(ConsoleRegion::CnBeijing, ConsoleSite::Domestic),
            Gateway {
                host: "bailian-cs.console.aliyun.com",
                action: "BroadScopeAspnGateway"
            }
        );
        assert_eq!(
            gateway_for(ConsoleRegion::CnBeijing, ConsoleSite::International),
            Gateway {
                host: "bailian-cs.console.alibabacloud.com",
                action: "BroadScopeAspnGateway"
            }
        );
        assert_eq!(
            gateway_for(ConsoleRegion::ApSoutheast1, ConsoleSite::Domestic),
            Gateway {
                host: "modelstudio-cs.console.aliyun.com",
                action: "IntlBroadScopeAspnGateway"
            }
        );
        assert_eq!(
            gateway_for(ConsoleRegion::ApSoutheast1, ConsoleSite::International),
            Gateway {
                host: "bailian-singapore-cs.alibabacloud.com",
                action: "IntlBroadScopeAspnGateway"
            }
        );
    }

    /// Unknown wire values fall back to the CLI's default row rather than
    /// failing — the config file is the CLI's, not ours.
    #[test]
    fn unknown_site_and_region_fall_back_to_the_default_row() {
        assert_eq!(ConsoleSite::parse("weird"), ConsoleSite::Domestic);
        assert_eq!(ConsoleSite::parse(""), ConsoleSite::Domestic);
        assert_eq!(
            ConsoleSite::parse("INTERNATIONAL"),
            ConsoleSite::International
        );
        assert_eq!(
            ConsoleRegion::parse("eu-central-1"),
            ConsoleRegion::CnBeijing
        );
        assert_eq!(
            ConsoleRegion::parse("AP-SOUTHEAST-1"),
            ConsoleRegion::ApSoutheast1
        );
    }

    /// Every `/` in the api param must arrive as `%2F`, and the action and
    /// product ride beside it — the URL contract the gateway dispatches on.
    #[test]
    fn usage_path_encodes_every_slash_in_the_api_param() {
        let path = usage_path("BroadScopeAspnGateway");
        assert_eq!(
            path,
            "/cli/api.json?action=BroadScopeAspnGateway&product=sfm_bailian\
             &api=zeldaHttp.apikeyMgr.%2Ftokenplan%2Fpersonal%2Fapi%2Fv2%2Fusage"
        );
        assert_eq!(path.matches("%2F").count(), 5, "{path}");
        assert!(!USAGE_API_ENCODED.contains('/'));
    }

    #[test]
    fn params_body_carries_the_api_version_and_cornerstone_context() {
        let params: Value = serde_json::from_str(&params_body()).unwrap();
        assert_eq!(params["Api"], USAGE_API);
        assert_eq!(params["V"], "1.0");
        assert_eq!(params["Data"]["cornerstoneParam"]["protocol"], "V2");
        assert_eq!(params["Data"]["cornerstoneParam"]["console"], "ONE_CONSOLE");
        assert_eq!(params["Data"]["cornerstoneParam"]["productCode"], "p_efm");
        assert_eq!(params["Data"]["cornerstoneParam"]["switchUserType"], 3);
        assert_eq!(
            params["Data"]["cornerstoneParam"]["consoleSite"],
            "BAILIAN_ALIYUN"
        );
    }

    #[test]
    fn form_body_encodes_the_region_and_params_fields() {
        let body = form_body(ConsoleRegion::CnBeijing);
        assert!(body.starts_with("region=cn-beijing&params=%7B"), "{body}");
        assert!(
            body.contains("%22Api%22%3A%22zeldaHttp.apikeyMgr.%2Ftokenplan"),
            "{body}"
        );
        assert!(body.ends_with("%7D"), "{body}");
        assert!(
            form_body(ConsoleRegion::ApSoutheast1).starts_with("region=ap-southeast-1&"),
            "the region form field carries the wire spelling"
        );
    }

    /// The full double envelope, end to end: ratios in, percents out.
    #[test]
    fn parses_the_full_double_envelope_into_percent_windows() {
        let fields = parse_response(full_envelope().as_bytes()).unwrap();
        assert_eq!(fields.per5_hour_percentage, Some(0.4217));
        assert_eq!(fields.per1_week_percentage, Some(0.7356));
        assert_eq!(fields.per5_hour_reset_ms, Some(1_789_200_000_000));
        assert_eq!(fields.per1_week_reset_ms, Some(1_789_600_000_000));

        let snap = fields.to_snapshot().unwrap();
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 42);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 74);
        assert_eq!(
            snap.session.as_ref().unwrap().resets_at,
            DateTime::from_timestamp_millis(1_789_200_000_000)
        );
        assert_eq!(
            snap.session.as_ref().unwrap().window_duration,
            FIVE_HOUR_WINDOW
        );
        assert_eq!(snap.weekly.as_ref().unwrap().window_duration, WEEKLY_WINDOW);
    }

    #[test]
    fn shallower_envelopes_unwrap_the_same() {
        let two = serde_json::json!({
            "DataV2": { "data": {
                "per5HourPercentage": 0.5,
                "per1WeekPercentage": 0.25,
            }}
        });
        let fields = parse_response(two.to_string().as_bytes()).unwrap();
        assert_eq!(fields.per5_hour_percentage, Some(0.5));
        assert_eq!(fields.per1_week_percentage, Some(0.25));

        let one = serde_json::json!({
            "DataV2": {
                "per5HourPercentage": 0.5,
                "per1WeekPercentage": 0.25,
            }
        });
        let fields = parse_response(one.to_string().as_bytes()).unwrap();
        assert_eq!(fields.per5_hour_percentage, Some(0.5));
        assert_eq!(fields.per1_week_percentage, Some(0.25));

        let plain = serde_json::json!({
            "data": {
                "per5HourPercentage": 0.5,
                "per1WeekPercentage": 0.25,
            }
        });
        let fields = parse_response(plain.to_string().as_bytes()).unwrap();
        assert_eq!(fields.per5_hour_percentage, Some(0.5));
        assert_eq!(fields.per1_week_percentage, Some(0.25));
    }

    /// `DataV2?.data?.data` wins over the shallower spellings, exactly like
    /// the CLI's left-to-right `??` chain.
    #[test]
    fn deeper_envelope_beats_the_shallower_ones() {
        let v = serde_json::json!({
            "DataV2": { "data": { "data": { "per5HourPercentage": 0.11 },
                                   "per5HourPercentage": 0.22 },
                        "per5HourPercentage": 0.33 },
            "data": { "per5HourPercentage": 0.44 },
            "per5HourPercentage": 0.55
        });
        assert_eq!(
            unwrap_payload(&v)
                .get("per5HourPercentage")
                .and_then(Value::as_f64),
            Some(0.11)
        );

        // `DataV2?.data` is null: the chain skips to `DataV2` itself.
        let v = serde_json::json!({
            "DataV2": { "data": null, "per5HourPercentage": 0.33 },
            "data": { "per5HourPercentage": 0.44 }
        });
        assert_eq!(
            unwrap_payload(&v)
                .get("per5HourPercentage")
                .and_then(Value::as_f64),
            Some(0.33)
        );

        // No DataV2 at all: `data.data`, then `data`, then the root. The
        // `data.data ?? data` step picks `data.data` — one level only; it
        // does not dig a second `.data` out of it.
        let v = serde_json::json!({ "data": { "per5HourPercentage": 0.66 } });
        assert_eq!(
            unwrap_payload(&v)
                .get("per5HourPercentage")
                .and_then(Value::as_f64),
            Some(0.66)
        );
        let v = serde_json::json!({ "data": { "data": { "per5HourPercentage": 0.77 } } });
        assert_eq!(unwrap_payload(&v), v.get("data").unwrap());
        assert_eq!(unwrap_payload(&v).get("per5HourPercentage"), None);
        // The final `?? data` is the root object itself.
        let v = serde_json::json!({ "per5HourPercentage": 0.55 });
        assert_eq!(
            unwrap_payload(&v)
                .get("per5HourPercentage")
                .and_then(Value::as_f64),
            Some(0.55)
        );
        assert_eq!(unwrap_payload(&v), &v);
    }

    /// An absent percentage is no window at all — never a zero.
    #[test]
    fn absent_percentages_leave_the_window_out() {
        let fields = parse_response(
            br#"{"data":{"per1WeekPercentage":0.5,"per1WeekResetTime":1789600000000}}"#,
        )
        .unwrap();
        assert_eq!(fields.per5_hour_percentage, None);
        let snap = fields.to_snapshot().unwrap();
        assert!(snap.session.is_none(), "{snap:?}");
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 50);
    }

    #[test]
    fn ratio_math_rounds_after_the_times_hundred() {
        for (ratio, pct) in [(0.4217, 42), (0.005, 1), (0.0, 0), (1.0, 100), (0.995, 100)] {
            let fields = UsageFields {
                per5_hour_percentage: Some(ratio),
                per1_week_percentage: None,
                per5_hour_reset_ms: None,
                per1_week_reset_ms: None,
            };
            assert_eq!(
                fields
                    .to_snapshot()
                    .unwrap()
                    .session
                    .unwrap()
                    .utilization_pct,
                pct,
                "{ratio}"
            );
        }
    }

    /// NaN / negative / above 1 are a changed wire contract, and must not
    /// collapse into a confident 0%.
    #[test]
    fn invalid_ratios_are_schema_drift_never_zero() {
        for raw in ["NaN", "-0.1", "1.4", "2", "null_plus", "\"0.5\"", "true"] {
            let body = format!(r#"{{"data":{{"per5HourPercentage":{raw}}}}}"#);
            let err = parse_response(body.as_bytes()).unwrap_err();
            assert!(matches!(err, AppError::Schema(_)), "{raw}: {err:?}");
        }
    }

    #[test]
    fn invalid_reset_times_are_schema_drift() {
        for raw in ["-1", "\"soon\"", "1e30", "true"] {
            let body = format!(r#"{{"data":{{"per5HourResetTime":{raw}}}}}"#);
            assert!(
                parse_response(body.as_bytes()).is_err(),
                "{raw} must not parse"
            );
        }
    }

    #[test]
    fn null_optional_fields_count_as_absent() {
        let fields =
            parse_response(br#"{"data":{"per5HourPercentage":null,"per5HourResetTime":null}}"#)
                .unwrap();
        assert_eq!(fields.per5_hour_percentage, None);
        assert_eq!(fields.per5_hour_reset_ms, None);
    }

    /// `NotLogined` in any spelling funnels into the Credentials re-auth
    /// error naming the CLI's own command.
    #[test]
    fn not_logined_is_the_reauth_error() {
        for code in ["NotLogined", "NotLogined_1001", "FLOW.NotLogined"] {
            let body = format!(r#"{{"success":false,"errorCode":"{code}"}}"#);
            let err = parse_response(body.as_bytes()).unwrap_err();
            assert!(matches!(err, AppError::Credentials(_)), "{code}: {err:?}");
            assert!(
                err.to_string().contains("bl auth login --console"),
                "{code}: {err}"
            );
        }
    }

    /// Any other failure code is upstream drift with its own name.
    #[test]
    fn other_gateway_failures_are_schema_errors() {
        let err = parse_response(br#"{"success":false,"errorCode":"NoPermission"}"#).unwrap_err();
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");
        assert!(err.to_string().contains("NoPermission"), "{err}");
        // `success` absent or true is not a failure.
        assert!(parse_response(br#"{"data":{}}"#).is_ok());
        assert!(parse_response(br#"{"success":true,"data":{}}"#).is_ok());
    }

    #[test]
    fn non_json_is_schema_drift() {
        let err = parse_response(b"<html>login page</html>").unwrap_err();
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");
    }
}
