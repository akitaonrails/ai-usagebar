//! Nous Research renderer helpers for the display-safe account snapshot.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::vendor::VendorId;

use super::types::AccountSnapshot;

pub const DISPLAY_NAME: &str = "Nous Research";
pub const VENDOR_SHORT: &str = VendorId::NousResearch.short_name();
pub const DEFAULT_FORMAT: &str = "{nous_pct}% · {nous_renewal}";
pub const NEUTRAL_UNAVAILABLE: &str = "—";

pub fn build_placeholders(
    snapshot: &AccountSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let percentage = snapshot
        .usage_percent()
        .map(|value| crate::format::clamp_pct(value).to_string())
        .unwrap_or_else(|| NEUTRAL_UNAVAILABLE.into());
    let renewal = crate::countdown::format(snapshot.current_period_end, now);
    let plan = snapshot
        .plan
        .as_deref()
        .map(crate::display::sanitize_untrusted_field)
        .unwrap_or_else(|| NEUTRAL_UNAVAILABLE.into());
    let monthly = format_credit(snapshot.monthly_credits);
    let remaining = format_credit(snapshot.credits_remaining);
    let purchased = format_credit(snapshot.purchased_credits_remaining);
    let total_usable = format_credit(snapshot.total_usable_credits);
    let rollover = format_credit(snapshot.rollover_credits);
    let mut values = HashMap::new();
    values.insert("vendor", DISPLAY_NAME.into());
    values.insert("vendor_short", VENDOR_SHORT.into());
    values.insert("plan", plan.clone());
    values.insert("nous_plan", plan);
    values.insert("session_pct", percentage.clone());
    values.insert("weekly_pct", percentage.clone());
    values.insert("nous_pct", percentage);
    values.insert("session_reset", renewal.clone());
    values.insert("weekly_reset", renewal.clone());
    values.insert("nous_renewal", renewal);
    values.insert("nous_credits_remaining", remaining);
    values.insert("nous_monthly_credits", monthly);
    values.insert("nous_purchased_credits_remaining", purchased.clone());
    values.insert("nous_top_up_credits_remaining", purchased);
    values.insert("nous_total_usable_credits", total_usable);
    values.insert("nous_rollover_credits", rollover);
    values
}

pub fn render_tooltip(snapshot: &AccountSnapshot, now: DateTime<Utc>) -> String {
    let values = build_placeholders(snapshot, now);
    let mut lines = vec![DISPLAY_NAME.to_string()];
    if let Some(plan) = snapshot.plan.as_deref() {
        lines.push(format!(
            "Plan: {}",
            escape_text(&crate::display::sanitize_untrusted_field(plan))
        ));
    }
    if let Some(pct) = snapshot.usage_percent() {
        lines.push(format!("Usage: {}%", crate::format::clamp_pct(pct)));
    }
    if let Some(remaining) = snapshot.credits_remaining {
        lines.push(format!(
            "Subscription credits remaining: {}",
            format_credit(Some(remaining))
        ));
    }
    if let Some(purchased) = snapshot.purchased_credits_remaining {
        lines.push(format!(
            "Top-up credits remaining: {}",
            format_credit(Some(purchased))
        ));
    }
    if let Some(total_usable) = snapshot.total_usable_credits {
        lines.push(format!(
            "Total usable credits: {}",
            format_credit(Some(total_usable))
        ));
    }
    if let Some(monthly) = snapshot.monthly_credits {
        lines.push(format!("Monthly credits: {}", format_credit(Some(monthly))));
    }
    if let Some(rollover) = snapshot.rollover_credits {
        lines.push(format!(
            "Rollover credits: {}",
            format_credit(Some(rollover))
        ));
    }
    lines.push(format!("Renews: {}", values["nous_renewal"]));
    lines.join("\n")
}

pub fn render(
    outcome: &crate::vendor::VendorOutcome,
    snapshot: &AccountSnapshot,
    theme: &crate::theme::Theme,
    opts: &crate::vendor::RenderOpts,
    now: DateTime<Utc>,
) -> crate::waybar::WaybarOutput {
    let severity = crate::pango::severity_for(
        snapshot
            .usage_percent()
            .map(|value| i32::from(crate::format::clamp_pct(value)))
            .unwrap_or(0),
    );
    let mut values = build_placeholders(snapshot, now);
    for value in values.values_mut() {
        *value = crate::pango::escape(value);
    }
    let mut text = if opts.format.is_none() && snapshot.usage_percent().is_none() {
        values
            .get("nous_renewal")
            .cloned()
            .unwrap_or_else(|| NEUTRAL_UNAVAILABLE.into())
    } else {
        crate::format::substitute(opts.format.as_deref().unwrap_or(DEFAULT_FORMAT), &values)
    };
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map(|icon| format!("{} ", crate::pango::escape(icon)))
        .unwrap_or_default();
    let tooltip = opts
        .tooltip_format
        .as_deref()
        .map(|template| crate::format::substitute(template, &values))
        .unwrap_or_else(|| render_tooltip(snapshot, now));
    crate::waybar::WaybarOutput {
        text: crate::pango::color_span(
            crate::pango::severity_color(severity, theme),
            &format!("{icon}{text}"),
        ),
        tooltip,
        class: crate::waybar::Class::from(severity),
    }
}

fn format_credit(value: Option<f64>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return NEUTRAL_UNAVAILABLE.into();
    };
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let mut text = format!("{value:.6}");
        while text.ends_with('0') {
            text.pop();
        }
        text
    }
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::nous::types::parse_account;

    fn snapshot() -> AccountSnapshot {
        parse_account(&json!({
            "plan": "Pro",
            "monthly_credits": 1000.0,
            "credits_remaining": 760.0,
            "purchased_credits_remaining": 125.5,
            "total_usable_credits": 885.5,
            "rollover_credits": 40.0,
            "period_end": "2026-09-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn nous_identity_and_placeholders_are_exact() {
        let snap = snapshot();
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        let values = build_placeholders(&snap, now);
        assert_eq!(DISPLAY_NAME, "Nous Research");
        assert_eq!(VENDOR_SHORT, "nrs");
        assert_eq!(values["vendor"], "Nous Research");
        assert_eq!(values["vendor_short"], "nrs");
        assert_eq!(values["plan"], "Pro");
        assert_eq!(values["nous_plan"], "Pro");
        assert_eq!(values["nous_pct"], "24");
        assert_eq!(values["session_pct"], "24");
        assert_eq!(values["weekly_pct"], "24");
        assert_eq!(values["nous_monthly_credits"], "1000");
        assert_eq!(values["nous_credits_remaining"], "760");
        assert_eq!(values["nous_purchased_credits_remaining"], "125.5");
        assert_eq!(values["nous_top_up_credits_remaining"], "125.5");
        assert_eq!(values["nous_total_usable_credits"], "885.5");
        assert_eq!(values["nous_rollover_credits"], "40");
    }

    #[test]
    fn tooltip_separates_subscription_top_up_and_total_credits() {
        let snap = snapshot();
        let tooltip = render_tooltip(&snap, Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap());
        assert!(tooltip.contains("Subscription credits remaining: 760"));
        assert!(tooltip.contains("Top-up credits remaining: 125.5"));
        assert!(tooltip.contains("Total usable credits: 885.5"));
        assert!(tooltip.contains("Usage: 24%"));
    }

    #[test]
    fn default_format_omits_missing_percentage_instead_of_fabricating_zero() {
        let snap =
            parse_account(&json!({"plan":"Free", "period_end":"2026-09-01T00:00:00Z"})).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        let outcome = crate::vendor::VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::NousResearch(snap.clone()),
            stale: false,
            last_error: None,
            cache_age: None,
        };
        let rendered = render(
            &outcome,
            &snap,
            &crate::theme::Theme::default(),
            &crate::vendor::RenderOpts {
                format: None,
                tooltip_format: None,
                icon: None,
                pace_tolerance: 10,
                format_pace_color: false,
                tooltip_pace_pts: false,
            },
            now,
        )
        .text;
        assert!(!rendered.contains("0%"));
        assert!(!rendered.contains('%'));
        assert!(rendered.contains("15d 12h"));
        assert_eq!(
            build_placeholders(&snap, now)["nous_pct"],
            NEUTRAL_UNAVAILABLE
        );
    }

    #[test]
    fn unavailable_metrics_use_neutral_value_and_plan_text_is_sanitized() {
        let snap = parse_account(&json!({"plan":"<Pro>&\"", "monthly_credits": 100.0})).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        let values = build_placeholders(&snap, now);
        assert_eq!(values["nous_credits_remaining"], NEUTRAL_UNAVAILABLE);
        let tooltip = render_tooltip(&snap, now);
        assert!(!tooltip.contains("<Pro>"));
        assert!(tooltip.contains("&lt;Pro&gt;"));

        let bidi =
            parse_account(&json!({"plan":"Pro\u{202e}spoof", "monthly_credits": 100.0})).unwrap();
        assert!(!render_tooltip(&bidi, now).contains('\u{202e}'));
    }
}
