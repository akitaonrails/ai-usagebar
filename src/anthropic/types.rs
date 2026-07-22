//! Wire types for the Anthropic OAuth usage endpoint.
//!
//! Every field is `Option<T>` or has `#[serde(default)]` — the endpoint is
//! undocumented and the shape varies across plan tiers and over time. The
//! lossy `serde(default)` approach matches claudebar's jq pattern of
//! `.field // empty`.

use serde::{Deserialize, Serialize};

use crate::usage::{
    AnthropicSnapshot, Cents, ExtraUsage, ScopedWindow, UsageWindow, default_decimal_places,
};

/// Top-level response from `GET /api/oauth/usage`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct UsageResponse {
    #[serde(default)]
    pub five_hour: Option<Window>,
    #[serde(default)]
    pub seven_day: Option<Window>,
    #[serde(default)]
    pub seven_day_sonnet: Option<Window>,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsageBlock>,
    /// Newer per-limit array. Carries model-scoped weekly windows
    /// (`kind == "weekly_scoped"`, e.g. the Fable weekly cap) that have no
    /// dedicated `seven_day_*` field.
    #[serde(default)]
    pub limits: Vec<LimitEntry>,
}

/// One entry of the `limits[]` array. Only `weekly_scoped` entries with a
/// model display name are lifted into the snapshot; everything else in the
/// array duplicates `five_hour`/`seven_day`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitEntry {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "de_percent_opt")]
    pub percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub scope: Option<LimitScope>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitScope {
    #[serde(default)]
    pub model: Option<LimitModel>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitModel {
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A single usage window — `utilization` is `0..=100` (integer percent).
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct Window {
    #[serde(default, deserialize_with = "de_percent")]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Pay-as-you-go extra usage. Both money values are non-negative integer cents,
/// but the API sometimes returns integral floats (e.g. `0.0`). Missing values
/// remain absent so an enabled but incomplete block cannot manufacture $0.00.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExtraUsageBlock {
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default, deserialize_with = "de_opt_cents")]
    pub monthly_limit: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_cents")]
    pub used_credits: Option<i64>,
    /// ISO currency code (`"BRL"`, `"USD"`, …). Absent on older payloads,
    /// which were always formatted as `$`.
    #[serde(default, deserialize_with = "de_opt_currency")]
    pub currency: Option<String>,
    /// Minor-unit digits for both money fields. Gated at the parse boundary:
    /// an absurd scale would corrupt every formatted amount downstream.
    #[serde(default, deserialize_with = "de_opt_decimal_places")]
    pub decimal_places: Option<u32>,
}

/// Accept a plausible minor-unit scale (0..=6 covers every ISO 4217 currency;
/// the largest real exponent is 4). Integral floats are tolerated for the same
/// reason `de_opt_cents` tolerates them: this endpoint emits them (the #30
/// payload carries `used_credits: 14157.0`), and rejecting `2.0` would fail
/// the whole response over a value that is unambiguous. Null/absent falls back
/// per-currency in the conversion. Anything else is drift: a wire
/// `decimal_places: 100` would overflow the scale and mis-state every amount,
/// so it fails loudly as `⚠` instead.
fn de_opt_decimal_places<'de, D>(d: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let n = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        _ => None,
    };
    match n {
        Some(n) if (0..=6).contains(&n) => Ok(Some(n as u32)),
        _ => Err(serde::de::Error::custom(
            "decimal_places must be an integer in 0..=6",
        )),
    }
}

/// Gate the currency to a plausible ISO 4217 alpha code. The value is embedded
/// verbatim in Pango bar markup and in the `;;`-delimited desktop FORMAT
/// protocol, so an arbitrary string is an injection vector as well as drift;
/// three ASCII uppercase letters can be neither.
fn de_opt_currency<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()) => Ok(Some(s)),
        Some(s) => Err(serde::de::Error::custom(format!(
            "currency {s:?} is not an ISO 4217 alpha code"
        ))),
    }
}

fn de_opt_cents<'de, D>(d: D) -> std::result::Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= 0 {
                    Ok(Some(i))
                } else {
                    Err(serde::de::Error::custom("cents cannot be negative"))
                }
            } else if let Some(f) = n.as_f64() {
                const MAX_EXACT_F64_INT: f64 = (1_u64 << 53) as f64;
                if f.is_finite() && f.fract() == 0.0 && (0.0..=MAX_EXACT_F64_INT).contains(&f) {
                    Ok(Some(f as i64))
                } else {
                    Err(serde::de::Error::custom(
                        "cents must be a non-negative integer in range",
                    ))
                }
            } else {
                Err(serde::de::Error::custom("number out of i64 range"))
            }
        }
        other => Err(serde::de::Error::custom(format!(
            "expected number or null, got {other:?}"
        ))),
    }
}

/// Slack tolerated above 100. The endpoint occasionally reports a hair over its
/// own cap — `used/limit` rounding, or usage that landed just before the block
/// did — and `to_window` saturates that back to 100.
const PCT_SLACK: f64 = 1.0;

/// Gate a wire percentage into `0..=100` (+ [`PCT_SLACK`]).
///
/// Rejecting here rather than in `to_window` is deliberate: `to_window` is
/// infallible and `into_snapshot` has no error channel, so the parse boundary
/// is the only place a bad value can still become a loud failure. A rejection
/// surfaces as `AppError::Json` and reaches the user as `⚠` — never as a
/// number we invented. Past the slack the field simply isn't a percentage on
/// this scale (rescaled to per-mille, a raw counter, a sentinel), and clamping
/// it would paint a "100%" bar we cannot vouch for. Non-finite values matter
/// most: `f64::NAN as i32` is silently `0`.
fn checked_percent<E: serde::de::Error>(v: f64) -> std::result::Result<f64, E> {
    if !v.is_finite() {
        return Err(E::custom(format!("percentage {v} is not finite")));
    }
    if !(0.0..=100.0 + PCT_SLACK).contains(&v) {
        return Err(E::custom(format!("percentage {v} outside 0..=100")));
    }
    Ok(v)
}

fn de_percent<'de, D>(d: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    checked_percent(f64::deserialize(d)?)
}

fn de_percent_opt<'de, D>(d: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d)?
        .map(checked_percent)
        .transpose()
}

impl UsageResponse {
    /// Lift the wire response into our canonical [`AnthropicSnapshot`].
    ///
    /// `plan_label` is the rendered plan name ("Max 5x" etc.), derived from
    /// the credentials file (since the usage endpoint doesn't include it).
    pub fn into_snapshot(self, plan_label: String) -> AnthropicSnapshot {
        // Window durations are constants per claudebar:172-173.
        const SESSION: chrono::Duration = chrono::Duration::hours(5);
        const WEEKLY: chrono::Duration = chrono::Duration::days(7);

        fn to_window(w: Option<Window>, dur: chrono::Duration) -> UsageWindow {
            let Some(w) = w else {
                return UsageWindow {
                    utilization_pct: 0,
                    resets_at: None,
                    window_duration: dur,
                };
            };
            UsageWindow {
                // Round to nearest, matching claudebar's `| round` jq filter,
                // then absorb the overshoot `de_percent` deliberately lets
                // through (100.4 → 100) so the bar never renders past full.
                utilization_pct: (w.utilization.round() as i32).clamp(0, 100),
                resets_at: w
                    .resets_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
                window_duration: dur,
            }
        }

        let session = to_window(self.five_hour, SESSION);
        let weekly = to_window(self.seven_day, WEEKLY);
        let sonnet = self.seven_day_sonnet.map(|w| to_window(Some(w), WEEKLY));
        let extra = self.extra_usage.filter(|e| e.is_enabled).and_then(|e| {
            Some(ExtraUsage {
                // `monthly_limit: null` is semantic, not drift: the endpoint
                // sends it for plans with no spending cap (e.g. Pro), so it
                // maps to None instead of discarding the block — which hid
                // real credit spend (#30). Only an unusable `used_credits`
                // still drops it: without the spend there is nothing to show.
                limit: e.monthly_limit.map(Cents),
                spent: Cents(e.used_credits?),
                // Absent scale falls back per-currency (JPY has no minor
                // unit; everything observed without the field was cents).
                decimal_places: e
                    .decimal_places
                    .unwrap_or_else(|| default_decimal_places(e.currency.as_deref())),
                currency: e.currency,
            })
        });
        let scoped = self
            .limits
            .into_iter()
            .filter(|l| l.kind.as_deref() == Some("weekly_scoped"))
            .filter_map(|l| {
                let label = l.scope?.model?.display_name?;
                // Same `?` discipline as the label above: an entry without a
                // percentage is dropped, not defaulted. `unwrap_or(0.0)` drew a
                // confident "Fable 0%" bar under a real model name — a number
                // the API never sent, which is worse than no bar at all.
                let utilization = l.percent?;
                let window = to_window(
                    Some(Window {
                        utilization,
                        resets_at: l.resets_at,
                    }),
                    WEEKLY,
                );
                Some(ScopedWindow { label, window })
            })
            .collect();

        AnthropicSnapshot {
            plan: plan_label,
            session,
            weekly,
            sonnet,
            scoped,
            extra,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let raw = r#"{
            "five_hour":         {"utilization": 42.7, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day":         {"utilization": 27.0, "resets_at": "2026-05-30T12:00:00Z"},
            "seven_day_sonnet":  {"utilization":  4.2, "resets_at": "2026-05-30T12:00:00Z"},
            "extra_usage":       {"is_enabled": true, "monthly_limit": 5000, "used_credits": 250}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Max 5x".into());
        assert_eq!(snap.session.utilization_pct, 43); // rounded
        assert_eq!(snap.weekly.utilization_pct, 27);
        assert_eq!(snap.sonnet.as_ref().unwrap().utilization_pct, 4);
        let extra = snap.extra.as_ref().unwrap();
        assert_eq!(extra.limit, Some(Cents(5000)));
        assert_eq!(extra.spent.0, 250);
        assert!(snap.session.resets_at.is_some());
    }

    #[test]
    fn parses_weekly_scoped_limits() {
        // Real shape observed 2026-07-08: the Fable weekly cap only exists
        // inside `limits[]`; there is no `seven_day_fable` field.
        let raw = r#"{
            "five_hour": {"utilization": 10.0, "resets_at": "2026-07-08T22:59:59Z"},
            "seven_day": {"utilization": 55.0, "resets_at": "2026-07-10T10:59:59Z"},
            "limits": [
                {"kind": "session", "group": "session", "percent": 10,
                 "severity": "normal", "resets_at": "2026-07-08T22:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_all", "group": "weekly", "percent": 55,
                 "severity": "normal", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 84,
                 "severity": "warning", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                 "is_active": true}
            ]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped.len(), 1);
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
        assert!(snap.scoped[0].window.resets_at.is_some());
        // Unscoped entries never duplicate into `scoped`.
        assert_eq!(snap.weekly.utilization_pct, 55);
    }

    #[test]
    fn missing_limits_array_yields_empty_scoped() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.scoped.is_empty());
    }

    #[test]
    fn missing_sonnet_and_extra_are_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.sonnet.is_none());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn disabled_extra_usage_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0},
            "seven_day": {"utilization": 0},
            "extra_usage": {"is_enabled": false, "monthly_limit": 5000, "used_credits": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn enabled_extra_usage_without_spend_is_dropped() {
        // `used_credits` is the essential datum: without it there is nothing
        // truthful to display, so the block is dropped rather than inventing
        // a $0.00 spend.
        for raw in [
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000}}"#,
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":null}}"#,
        ] {
            let resp: UsageResponse = serde_json::from_str(raw).unwrap();
            assert!(resp.into_snapshot("Pro".into()).extra.is_none(), "{raw}");
        }
    }

    #[test]
    fn uncapped_plan_keeps_real_spend_visible() {
        // The #30 regression: the endpoint sends `monthly_limit: null` for
        // plans with no spending cap (e.g. Pro). Discarding the block hid
        // genuine credit spend — this fixture is the reporter's actual cached
        // response (R$ 141.57, an integral float).
        let resp: UsageResponse = serde_json::from_str(
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":null,
                "used_credits":14157.0,"currency":"BRL","decimal_places":2,
                "disabled_reason":null}}"#,
        )
        .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, None);
        assert_eq!(extra.spent.0, 14157);
        // No denominator → no invented percentage; bar stays calm.
        assert_eq!(extra.percent(), 0);
        // The block's own currency and scale propagate, so the renderer can
        // say R$141.57 instead of claiming `$` for reais.
        assert_eq!(extra.currency.as_deref(), Some("BRL"));
        assert_eq!(extra.decimal_places, 2);
        assert_eq!(extra.fmt_spent(), "R$141.57");

        // An *absent* limit renders the same way: with `#[serde(default)]`,
        // absent and explicit null are indistinguishable at the struct level,
        // and hiding real spend because a secondary field went missing is the
        // exact failure mode of #30. Nothing is fabricated either way — the
        // spend shown is exactly what the API sent.
        let resp: UsageResponse =
            serde_json::from_str(r#"{"extra_usage":{"is_enabled":true,"used_credits":250}}"#)
                .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, None);
        assert_eq!(extra.spent.0, 250);
    }

    #[test]
    fn implausible_decimal_places_is_schema_drift() {
        // A wire scale outside 0..=6 would mis-state every formatted amount
        // (10^100 overflows outright), so it fails loudly instead.
        for value in ["7", "-1", "100", "2.5"] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"used_credits":250,"decimal_places":{value}}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }
        // An integral float is fine — this endpoint floats its numbers (the
        // #30 payload has `used_credits: 14157.0`), and rejecting 2.0 would
        // fail the whole response over an unambiguous value. Worse: the fetch
        // caches the body BEFORE parsing, so a rejected response would evict
        // the last good payload and leave a persistent ⚠.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"decimal_places":2.0}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into())
                .extra
                .unwrap()
                .decimal_places,
            2
        );
        // Null and absent both fall back to the historical cent scale.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"decimal_places":null}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into())
                .extra
                .unwrap()
                .decimal_places,
            2
        );
        // ...except for a currency with no minor unit, where "2" would divide
        // every amount by 100 (¥500 rendered as ¥5.00).
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":500,"currency":"JPY"}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.decimal_places, 0);
        assert_eq!(extra.fmt_spent(), "¥500");
    }

    #[test]
    fn currency_must_be_an_iso_alpha_code() {
        // The value lands verbatim in Pango markup and in the `;;`-delimited
        // desktop FORMAT protocol, so anything but three ASCII uppercase
        // letters is rejected as drift — it is an injection vector besides.
        for value in [r#""brl""#, r#""""#, r#""R$""#, r#""USD;;0""#, r#""<b>""#] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"used_credits":250,"currency":{value}}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }
        // Null stays acceptable — same as absent.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"currency":null}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into()).extra.unwrap().currency,
            None
        );
    }

    #[test]
    fn malformed_cent_values_are_schema_drift() {
        for value in ["-1", "1.5", "1e300", "true", r#""lots""#] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"monthly_limit":{value},"used_credits":0}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }

        let resp: UsageResponse = serde_json::from_str(
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000.0,"used_credits":250.0}}"#,
        )
        .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, Some(Cents(5000)));
        assert_eq!(extra.spent.0, 250);
    }

    #[test]
    fn empty_object_yields_neutral_snapshot() {
        let resp: UsageResponse = serde_json::from_str("{}").unwrap();
        let snap = resp.into_snapshot("Unknown".into());
        assert_eq!(snap.session.utilization_pct, 0);
        assert_eq!(snap.weekly.utilization_pct, 0);
        assert!(snap.session.resets_at.is_none());
    }

    #[test]
    fn benign_overshoot_saturates_to_hundred() {
        // A hair over the cap is rounding noise, not drift — it must render as
        // a full bar, not break the widget.
        let raw = r#"{
            "five_hour": {"utilization": 100.4},
            "seven_day": {"utilization": 100.6}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.session.utilization_pct, 100);
        assert_eq!(snap.weekly.utilization_pct, 100); // rounds to 101, saturated
    }

    #[test]
    fn out_of_range_utilization_is_rejected_not_clamped() {
        for raw in [
            r#"{"five_hour": {"utilization": 500}}"#,
            r#"{"five_hour": {"utilization": -1}}"#,
            r#"{"seven_day": {"utilization": 101.5}}"#,
        ] {
            let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
            assert!(err.to_string().contains("outside 0..=100"), "{raw}: {err}");
        }
    }

    #[test]
    fn out_of_range_scoped_percent_is_rejected() {
        // `limits[].percent` reaches the same bar via the synthesized Window.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 420,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
        assert!(err.to_string().contains("outside 0..=100"), "{err}");
    }

    #[test]
    fn non_finite_percentage_is_rejected() {
        // `f64::NAN as i32` is silently 0 — the fabricated number this gate
        // exists to prevent. JSON has no NaN literal, so drive it directly.
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(checked_percent::<serde_json::Error>(v).is_err(), "{v}");
        }
    }

    #[test]
    fn scoped_limit_without_a_percentage_is_dropped_not_zeroed() {
        // The gate rejects impossible numbers; an absent one is not a parse
        // failure. But it must not become a bar either: `unwrap_or(0.0)` drew a
        // confident "Fable 0%" under a real model name that the API never sent.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": null,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(
            snap.scoped.is_empty(),
            "a scoped limit with no percentage must not render a fabricated bar"
        );

        // A real percentage still produces the window, so the drop is targeted.
        let ok = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 84,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(ok).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
    }

    #[test]
    fn unparseable_reset_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 50, "resets_at": "not a date"},
            "seven_day": {"utilization": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.session.resets_at.is_none());
        assert_eq!(snap.session.utilization_pct, 50);
    }
}
