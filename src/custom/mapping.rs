//! Project an arbitrary JSON body onto a [`CustomSnapshot`] through the
//! pointers of a [`CustomProviderConfig`].
//!
//! Pure: no I/O and no clock, so the tests can cover every rule with a
//! literal body. The body is untrusted — it came from whatever URL the user
//! configured — so every string that leaves this module has been through
//! `sanitize_untrusted_line` and capped, and no error message quotes the body:
//! an error names the pointer that failed and the shape it found, which is
//! what the user needs to fix their config and nothing a hostile endpoint can
//! use to put text of its own choosing into the bar.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::config::{CustomMetricSpec, CustomProviderConfig, CustomTextSpec};
use crate::display::sanitize_untrusted_line;
use crate::error::{AppError, Result};

use super::types::{CustomMetric, CustomSnapshot, CustomText};

/// Longest string kept from a response. Plan names and text values are a
/// few dozen characters; anything longer is a body that was pointed at by
/// mistake, and a Waybar tooltip is the wrong place to find that out.
pub const MAX_STRING_CHARS: usize = 200;

/// Epoch values at or above this are milliseconds. 1e11 seconds is the year
/// 5138 and 1e11 milliseconds is March 1973, so no reset time ever sits
/// legitimately on the wrong side of the line.
const EPOCH_MS_THRESHOLD: f64 = 1e11;

pub fn project(body: &Value, spec: &CustomProviderConfig) -> Result<CustomSnapshot> {
    let id = spec.id.as_str();
    let plan = match spec.plan_path.as_deref() {
        Some(pointer) => Some(read_string(body, id, pointer)?),
        None => spec.plan.as_deref().map(clean),
    };
    let metrics = spec
        .metrics
        .iter()
        .map(|m| project_metric(body, id, m))
        .collect::<Result<Vec<_>>>()?;
    let texts = spec
        .texts
        .iter()
        .map(|t| project_text(body, id, t))
        .collect::<Result<Vec<_>>>()?;
    Ok(CustomSnapshot {
        plan,
        metrics,
        texts,
    })
}

fn project_metric(body: &Value, id: &str, spec: &CustomMetricSpec) -> Result<CustomMetric> {
    let (pct, footnote) = match (&spec.percent, &spec.used, &spec.limit) {
        (Some(pointer), _, _) => (clamp_pct(read_number(body, id, pointer)?), String::new()),
        (None, Some(used_ptr), Some(limit_ptr)) => {
            let used = read_number(body, id, used_ptr)?;
            let limit = read_number(body, id, limit_ptr)?;
            if limit <= 0.0 {
                return Err(AppError::Schema(format!(
                    "custom {id}: {limit_ptr} must be greater than zero"
                )));
            }
            (
                clamp_pct(used / limit * 100.0),
                format!("{} of {}", format_number(used), format_number(limit)),
            )
        }
        // `Config::validate` already rejects this shape; the arm is here so
        // a spec built in code cannot reach a bar with a percentage of nothing.
        _ => {
            return Err(AppError::Schema(format!(
                "custom {id}: metric {:?} needs `percent`, or both `used` and `limit`",
                spec.label
            )));
        }
    };
    let resets_at = match spec.resets_at.as_deref() {
        Some(pointer) => Some(read_timestamp(body, id, pointer)?),
        None => None,
    };
    Ok(CustomMetric {
        label: clean(&spec.label),
        pct,
        footnote,
        resets_at,
        window_secs: spec.window_secs,
    })
}

fn project_text(body: &Value, id: &str, spec: &CustomTextSpec) -> Result<CustomText> {
    let value = match lookup(body, id, &spec.value)? {
        Value::String(s) => clean(s),
        Value::Number(n) => format_json_number(n, id, &spec.value)?,
        Value::Bool(b) => b.to_string(),
        other => {
            return Err(wrong_type(
                id,
                &spec.value,
                "a string, number, or boolean",
                other,
            ));
        }
    };
    Ok(CustomText {
        label: clean(&spec.label),
        value,
    })
}

fn lookup<'a>(body: &'a Value, id: &str, pointer: &str) -> Result<&'a Value> {
    body.pointer(pointer)
        .ok_or_else(|| AppError::Schema(format!("custom {id}: {pointer} is missing")))
}

fn wrong_type(id: &str, pointer: &str, expected: &str, found: &Value) -> AppError {
    AppError::Schema(format!(
        "custom {id}: {pointer} is {}, expected {expected}",
        describe(found)
    ))
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn read_string(body: &Value, id: &str, pointer: &str) -> Result<String> {
    match lookup(body, id, pointer)? {
        Value::String(s) => Ok(clean(s)),
        other => Err(wrong_type(id, pointer, "a string", other)),
    }
}

/// A JSON number, or a string holding one. Providers that serialise counters
/// as strings ("1234") are common enough to accept; digit grouping ("1,234")
/// is not a number and is refused rather than silently read as 1.
fn read_number(body: &Value, id: &str, pointer: &str) -> Result<f64> {
    match lookup(body, id, pointer)? {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
            AppError::Schema(format!("custom {id}: {pointer} is not a finite number"))
        }),
        Value::String(s) => parse_numeric_string(s).ok_or_else(|| {
            AppError::Schema(format!(
                "custom {id}: {pointer} is a string that does not hold a number"
            ))
        }),
        other => Err(wrong_type(id, pointer, "a number", other)),
    }
}

/// `str::parse::<f64>` accepts "inf", "nan" and "infinity"; a quota is none
/// of those, so only the characters of a decimal literal get through.
fn parse_numeric_string(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn read_timestamp(body: &Value, id: &str, pointer: &str) -> Result<DateTime<Utc>> {
    let parsed = match lookup(body, id, pointer)? {
        Value::String(s) => match DateTime::parse_from_rfc3339(s.trim()) {
            Ok(t) => Some(t.with_timezone(&Utc)),
            Err(_) => parse_numeric_string(s).and_then(epoch_to_datetime),
        },
        Value::Number(n) => n.as_f64().and_then(epoch_to_datetime),
        other => {
            return Err(wrong_type(
                id,
                pointer,
                "an RFC 3339 string or a Unix epoch",
                other,
            ));
        }
    };
    parsed.ok_or_else(|| {
        AppError::Schema(format!(
            "custom {id}: {pointer} is not an RFC 3339 timestamp or a Unix epoch"
        ))
    })
}

fn epoch_to_datetime(n: f64) -> Option<DateTime<Utc>> {
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    if n < EPOCH_MS_THRESHOLD {
        DateTime::from_timestamp(n.trunc() as i64, 0)
    } else {
        DateTime::from_timestamp_millis(n.trunc() as i64)
    }
}

fn clamp_pct(v: f64) -> u16 {
    if v.is_nan() {
        0
    } else {
        v.round().clamp(0.0, 100.0) as u16
    }
}

/// Integers print bare, everything else with up to two decimals: a footnote
/// reads "12 of 100", not "12.00 of 100.00", and "12.5 of 100" keeps the half
/// a provider reported without inventing a trailing zero.
pub fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Integers that fit the JSON number as written keep every digit; `as_f64`
/// would round a 19-digit account id, and a text row is where such ids go.
fn format_json_number(n: &serde_json::Number, id: &str, pointer: &str) -> Result<String> {
    if n.is_i64() || n.is_u64() {
        return Ok(n.to_string());
    }
    n.as_f64()
        .filter(|f| f.is_finite())
        .map(format_number)
        .ok_or_else(|| AppError::Schema(format!("custom {id}: {pointer} is not a finite number")))
}

fn clean(s: &str) -> String {
    sanitize_untrusted_line(s)
        .chars()
        .take(MAX_STRING_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metric(label: &str, used: &str, limit: &str) -> CustomMetricSpec {
        CustomMetricSpec {
            label: label.into(),
            used: Some(used.into()),
            limit: Some(limit.into()),
            ..CustomMetricSpec::default()
        }
    }

    fn pct_metric(label: &str, percent: &str) -> CustomMetricSpec {
        CustomMetricSpec {
            label: label.into(),
            percent: Some(percent.into()),
            ..CustomMetricSpec::default()
        }
    }

    fn spec(metrics: Vec<CustomMetricSpec>, texts: Vec<CustomTextSpec>) -> CustomProviderConfig {
        CustomProviderConfig {
            id: "mytool".into(),
            metrics,
            texts,
            ..CustomProviderConfig::default()
        }
    }

    fn only_metric(spec: &CustomProviderConfig, body: &Value) -> CustomMetric {
        project(body, spec).unwrap().metrics.remove(0)
    }

    #[test]
    fn used_and_limit_become_a_rounded_percent_with_a_footnote() {
        let spec = spec(vec![metric("Requests", "/used", "/limit")], vec![]);
        let m = only_metric(&spec, &json!({"used": 12, "limit": 100}));
        assert_eq!(m.pct, 12);
        assert_eq!(m.footnote, "12 of 100");

        let m = only_metric(&spec, &json!({"used": 2, "limit": 3}));
        assert_eq!(m.pct, 67, "2/3 rounds to 67, not 66");
    }

    #[test]
    fn used_over_limit_clamps_to_100() {
        let spec = spec(vec![metric("Requests", "/used", "/limit")], vec![]);
        let m = only_metric(&spec, &json!({"used": 250, "limit": 100}));
        assert_eq!(m.pct, 100);
        assert_eq!(m.footnote, "250 of 100");
    }

    #[test]
    fn percent_pointer_is_taken_directly_and_clamped() {
        let spec = spec(vec![pct_metric("Quota", "/pct")], vec![]);
        let m = only_metric(&spec, &json!({"pct": 42.4}));
        assert_eq!(m.pct, 42);
        assert_eq!(m.footnote, "", "a bare percentage has no denominator");
        assert_eq!(only_metric(&spec, &json!({"pct": 150})).pct, 100);
        assert_eq!(only_metric(&spec, &json!({"pct": -5})).pct, 0);
    }

    #[test]
    fn numeric_strings_are_accepted_and_grouped_digits_are_not() {
        let spec = spec(vec![metric("Requests", "/used", "/limit")], vec![]);
        let m = only_metric(&spec, &json!({"used": " 12 ", "limit": "100"}));
        assert_eq!(m.pct, 12);

        let err = project(&json!({"used": "1,234", "limit": "100"}), &spec).unwrap_err();
        assert!(err.to_string().contains("/used"), "{err}");
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");

        let err = project(&json!({"used": "inf", "limit": "100"}), &spec).unwrap_err();
        assert!(err.to_string().contains("/used"), "{err}");
    }

    #[test]
    fn a_zero_or_negative_limit_is_a_schema_error() {
        let spec = spec(vec![metric("Requests", "/used", "/limit")], vec![]);
        for limit in [0, -1] {
            let err = project(&json!({"used": 1, "limit": limit}), &spec).unwrap_err();
            assert!(matches!(err, AppError::Schema(_)), "{err:?}");
            assert!(err.to_string().contains("/limit"), "{err}");
        }
    }

    #[test]
    fn footnote_prints_integers_bare_and_fractions_to_two_places() {
        let spec = spec(vec![metric("Spend", "/used", "/limit")], vec![]);
        let m = only_metric(&spec, &json!({"used": 12.5, "limit": 100.0}));
        assert_eq!(m.footnote, "12.5 of 100");
        let m = only_metric(&spec, &json!({"used": 12.345, "limit": 1000000}));
        assert_eq!(m.footnote, "12.35 of 1000000");
        assert_eq!(format_number(0.001), "0");
        assert_eq!(format_number(-3.0), "-3");
    }

    #[test]
    fn resets_at_accepts_rfc3339_and_epoch_seconds_and_milliseconds() {
        let spec = spec(
            vec![CustomMetricSpec {
                resets_at: Some("/reset".into()),
                ..metric("Requests", "/used", "/limit")
            }],
            vec![],
        );
        let expected = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for reset in [
            json!("2026-01-01T00:00:00Z"),
            json!("2026-01-01T01:00:00+01:00"),
            json!(1_767_225_600),
            json!(1_767_225_600_000u64),
            json!("1767225600"),
        ] {
            let body = json!({"used": 1, "limit": 2, "reset": reset});
            assert_eq!(
                only_metric(&spec, &body).resets_at,
                Some(expected),
                "{reset}"
            );
        }
    }

    #[test]
    fn an_unparsable_resets_at_names_the_pointer() {
        let spec = spec(
            vec![CustomMetricSpec {
                resets_at: Some("/reset".into()),
                ..metric("Requests", "/used", "/limit")
            }],
            vec![],
        );
        for reset in [json!("next tuesday"), json!(-1), json!(true)] {
            let body = json!({"used": 1, "limit": 2, "reset": reset});
            let err = project(&body, &spec).unwrap_err();
            assert!(matches!(err, AppError::Schema(_)), "{err:?}");
            assert!(err.to_string().contains("/reset"), "{err}");
        }
    }

    #[test]
    fn a_missing_pointer_names_the_pointer_and_not_the_body() {
        let spec = spec(
            vec![metric("Requests", "/usage/used", "/usage/limit")],
            vec![],
        );
        let err = project(&json!({"secret": "hunter2"}), &spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("custom mytool: /usage/used is missing"),
            "{msg}"
        );
        assert!(!msg.contains("hunter2"), "{msg}");
    }

    #[test]
    fn a_wrong_type_pointer_is_rejected() {
        let spec = spec(vec![metric("Requests", "/used", "/limit")], vec![]);
        let err = project(&json!({"used": {"n": 1}, "limit": 100}), &spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("/used is an object, expected a number"),
            "{msg}"
        );
    }

    #[test]
    fn text_values_render_strings_numbers_and_booleans() {
        let texts = vec![
            CustomTextSpec {
                label: "Tier".into(),
                value: "/tier".into(),
            },
            CustomTextSpec {
                label: "Seats".into(),
                value: "/seats".into(),
            },
            CustomTextSpec {
                label: "Spend".into(),
                value: "/spend".into(),
            },
            CustomTextSpec {
                label: "Active".into(),
                value: "/active".into(),
            },
            CustomTextSpec {
                label: "Account".into(),
                value: "/account".into(),
            },
        ];
        let spec = spec(vec![], texts);
        let body = json!({
            "tier": "Pro",
            "seats": 5,
            "spend": 12.5,
            "active": true,
            "account": 12345678901234567890u64
        });
        let snap = project(&body, &spec).unwrap();
        let values: Vec<&str> = snap.texts.iter().map(|t| t.value.as_str()).collect();
        assert_eq!(values, ["Pro", "5", "12.5", "true", "12345678901234567890"]);
        assert_eq!(snap.texts[0].label, "Tier");
    }

    #[test]
    fn a_null_or_missing_text_value_is_a_schema_error() {
        let spec = spec(
            vec![],
            vec![CustomTextSpec {
                label: "Tier".into(),
                value: "/tier".into(),
            }],
        );
        let err = project(&json!({"tier": null}), &spec).unwrap_err();
        assert!(err.to_string().contains("/tier is null"), "{err}");
        let err = project(&json!({}), &spec).unwrap_err();
        assert!(err.to_string().contains("/tier is missing"), "{err}");
    }

    #[test]
    fn text_values_are_sanitized_and_capped() {
        let spec = spec(
            vec![],
            vec![CustomTextSpec {
                label: "Tier".into(),
                value: "/tier".into(),
            }],
        );
        let snap = project(&json!({"tier": "\u{1b}[31mred\u{7}\nline"}), &spec).unwrap();
        assert_eq!(snap.texts[0].value, "[31mred line");

        let long = "x".repeat(MAX_STRING_CHARS * 3);
        let snap = project(&json!({"tier": long}), &spec).unwrap();
        assert_eq!(snap.texts[0].value.chars().count(), MAX_STRING_CHARS);
    }

    #[test]
    fn plan_path_wins_over_the_literal_plan() {
        let mut spec = spec(vec![pct_metric("Quota", "/pct")], vec![]);
        spec.plan = Some("Literal".into());
        assert_eq!(
            project(&json!({"pct": 1}), &spec).unwrap().plan.as_deref(),
            Some("Literal")
        );

        spec.plan_path = Some("/plan".into());
        let body = json!({"pct": 1, "plan": "Team \u{1b}[0m"});
        assert_eq!(
            project(&body, &spec).unwrap().plan.as_deref(),
            Some("Team [0m")
        );

        let err = project(&json!({"pct": 1, "plan": 7}), &spec).unwrap_err();
        assert!(err.to_string().contains("/plan is a number"), "{err}");
    }

    #[test]
    fn window_secs_and_labels_travel_through() {
        let spec = spec(
            vec![CustomMetricSpec {
                window_secs: Some(3600),
                ..metric("Hourly", "/used", "/limit")
            }],
            vec![],
        );
        let m = only_metric(&spec, &json!({"used": 1, "limit": 4}));
        assert_eq!(m.label, "Hourly");
        assert_eq!(m.window_secs, Some(3600));
        assert_eq!(m.resets_at, None);
    }

    #[test]
    fn the_snapshot_round_trips_through_serde_for_the_cache() {
        let spec = spec(
            vec![CustomMetricSpec {
                resets_at: Some("/reset".into()),
                ..metric("Requests", "/used", "/limit")
            }],
            vec![CustomTextSpec {
                label: "Tier".into(),
                value: "/tier".into(),
            }],
        );
        let body = json!({"used": 1, "limit": 4, "reset": 1_767_225_600, "tier": "Pro"});
        let snap = project(&body, &spec).unwrap();
        let bytes = serde_json::to_vec(&snap).unwrap();
        let back: CustomSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, snap);
    }
}
