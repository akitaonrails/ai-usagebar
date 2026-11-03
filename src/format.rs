//! `{placeholder}` substitution for `--format` and `--tooltip-format`.
//!
//! Same surface as claudebar (claudebar:625-667): placeholders are surrounded
//! by `{}`, unknown placeholders are left untouched (matching bash parameter
//! expansion's default behavior — claudebar uses `${text//\{x\}/$val}` which
//! is a no-op for unknown keys).
//!
//! Built on a `Map<&str, String>` so each vendor can register its own
//! placeholder set and the rendering code doesn't need to know what they are.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};

use crate::countdown;
use crate::pacing;
use crate::usage::{ResetCredit, ResetCredits, UsageWindow};
use crate::vendor::RenderOpts;

/// The five per-window placeholder values every windowed renderer registers
/// (`pct`, `reset`, `elapsed` and the two pace glyphs). A window the account
/// does not report yields empty strings, the missing-placeholder convention.
#[derive(Default)]
pub struct WindowPlaceholders {
    pub pct: String,
    pub reset: String,
    pub elapsed: String,
    pub ratio_pace: String,
    pub point_pace: String,
}

pub fn window_placeholders(
    window: Option<&UsageWindow>,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WindowPlaceholders {
    let Some(window) = window else {
        return WindowPlaceholders::default();
    };
    let pace = pacing::calc(
        window.utilization_pct,
        window.resets_at,
        now,
        window.window_duration,
        opts.pace_tolerance,
    );
    WindowPlaceholders {
        pct: window.utilization_pct.to_string(),
        reset: countdown::format(window.resets_at, now),
        elapsed: pace.elapsed_pct.to_string(),
        ratio_pace: pace.ratio_pace.glyph().to_string(),
        point_pace: pace.point_pace.glyph().to_string(),
    }
}

/// A monetary amount, with the sign outside the symbol. `format!("${v:.2}")`
/// puts it inside — `$-5.71` — which reads as a typo rather than as debt, and
/// several providers let a balance go negative: OpenRouter overruns its
/// credits, Moonshot carries an explicit `cash_balance` debt.
///
/// The sign is decided from the *rounded* magnitude, so neither a negative
/// zero off the wire nor a sub-cent debt can produce the nonsense `-$0.00`.
///
/// This is the one place that decides what money looks like. Every renderer
/// goes through it so a debt cannot be spelled two ways in two panels.
pub fn money(v: f64, currency: &str) -> String {
    let magnitude = format!("{:.2}", v.abs());
    let sign = if v < 0.0 && magnitude != "0.00" {
        "-"
    } else {
        ""
    };
    with_currency(sign, &magnitude, Some(currency))
}

/// Attach a currency to an already-formatted magnitude and sign.
///
/// The one table that decides which currencies get a symbol. [`money`] works in
/// `f64` at two decimals and [`crate::usage::fmt_minor`] works in integer minor
/// units at the currency's own scale — two different numbers, but they must not
/// disagree about what a euro looks like. They did: `money` rendered EUR as
/// `3.50 EUR` while `fmt_minor` rendered the same currency as `€3.50`.
///
/// `None` means a payload that predates any currency field; those were always
/// USD. A code with no symbol here trails the code instead of guessing one,
/// which is still truthful — rendering R$ 141.57 as "$141.57" is a claim about
/// the wrong currency, the same class of defect as a fabricated number.
pub fn with_currency(sign: &str, number: &str, currency: Option<&str>) -> String {
    match currency {
        None | Some("USD") => format!("{sign}${number}"),
        Some("BRL") => format!("{sign}R${number}"),
        Some("EUR") => format!("{sign}€{number}"),
        Some("GBP") => format!("{sign}£{number}"),
        Some("JPY") | Some("CNY") => format!("{sign}¥{number}"),
        Some(other) => format!("{sign}{number} {other}"),
    }
}

/// Upper-case the first character, leaving the rest alone.
///
/// Vendor plans arrive lower-cased (`"pro"`, `"max"`, `"glm coding pro"`) and
/// every one of them wants the same title-ish label. `char::to_uppercase` can
/// yield more than one char, so this is not `s[..1].to_uppercase() + &s[1..]`.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

/// [`money`] for the providers that only ever bill in dollars.
pub fn usd(v: f64) -> String {
    money(v, "USD")
}

/// A token count in the compact form providers use on their own dashboards:
/// `900`, `12.4K`, `812.4M`, `2.5B`.
///
/// One decimal wherever a unit applies, dropped when it would be `.0` so a
/// round allowance reads `500M` rather than `500.0M`. Token allowances arrive
/// in the billions; spelling them out pushes every meter off the row.
pub fn tokens(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let magnitude = (v as f64).abs();
    for (scale, suffix) in [(1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if magnitude >= scale {
            let mut text = format!("{:.1}", magnitude / scale);
            if text.ends_with(".0") {
                text.truncate(text.len() - 2);
            }
            return format!("{sign}{text}{suffix}");
        }
    }
    format!("{v}")
}

/// A raw percentage as a whole number a meter can draw: rounded, held to
/// 0–100, and NaN-safe.
///
/// A meter whose denominator came off the wire can be handed a NaN
/// (`0.0 / 0.0`) or an out-of-range ratio, and each caller inventing its own
/// guard is how two gauges end up disagreeing about what "100%" means.
///
/// **A shared helper, not a chokepoint.** Every float percentage that is
/// rounded and held to 0–100 routes through here, and an `i32` caller widens
/// the result with `i32::from`. Nothing enforces that. Some percentages follow
/// a different rule on purpose and do not come here: a whole-number percent
/// that a parser already rounded is clamped as an integer before it is cast
/// into `Metric.pct`, and a few vendors round without clamping, or reject an
/// out-of-range value instead of holding it. Prefer this for new callers.
///
/// # Examples
///
/// ```
/// use ai_usagebar::format::clamp_pct;
///
/// assert_eq!(clamp_pct(24.5), 25);
/// assert_eq!(clamp_pct(-3.0), 0);
/// assert_eq!(clamp_pct(140.0), 100);
/// assert_eq!(clamp_pct(f64::NAN), 0);
/// ```
pub fn clamp_pct(v: f64) -> u16 {
    if v.is_nan() {
        0
    } else {
        v.round().clamp(0.0, 100.0) as u16
    }
}

pub fn local_time_hm(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local).format("%H:%M").to_string()
}

pub fn local_time_hms(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub fn local_date_hm(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local)
        .format("%b %-d %H:%M")
        .to_string()
}

/// Compact count for placeholders and tooltips that have to stay on one line.
pub fn reset_credits(credits: &ResetCredits) -> String {
    let noun = if credits.available == 1 {
        "reset"
    } else {
        "resets"
    };
    format!("{} {noun} available", credits.available)
}

/// One row per banked reset, soonest expiry first. This is what the panel,
/// TUI, and tooltip list — a single "2 available · next expires Oct 4" line
/// hides that two credits can lapse hours apart on the same day.
pub fn reset_credit_lines(credits: &ResetCredits, now: DateTime<Utc>) -> Vec<String> {
    let mut items = credits.credits.clone();
    items.sort_by_key(|credit| credit.expires_at);
    let mut lines: Vec<String> = items
        .iter()
        .map(|credit| reset_credit_line(credit, now))
        .collect();
    if lines.is_empty() {
        // A stated zero still gets a row: an empty block reads as missing
        // data rather than "you hold none".
        lines.push(if credits.available > 0 {
            reset_credits(credits)
        } else {
            "None available".to_string()
        });
    }
    lines
}

fn reset_credit_line(credit: &ResetCredit, now: DateTime<Utc>) -> String {
    let expiry = match credit.expires_at {
        Some(expires) if expires <= now => format!("expired {}", local_date_hm(expires)),
        Some(expires) => format!(
            "expires {} ({})",
            local_date_hm(expires),
            crate::countdown::format(Some(expires), now)
        ),
        None => "no expiry reported".into(),
    };
    match credit
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => format!("{title} · {expiry}"),
        None => capitalize(&expiry),
    }
}

/// The spending-page pair for one Cursor credit grant: `$21.00/$25.00
/// remaining`, plus the expiry when the grant has one.
///
/// Money goes through [`crate::usage::fmt_minor`] so the sign and the dollar
/// symbol match every other USD figure. The date is the local clock time, the
/// same way a banked reset is written.
pub fn cursor_credit_line(
    remaining_cents: i64,
    total_cents: i64,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> String {
    let pair = format!(
        "{}/{}",
        crate::usage::fmt_minor(remaining_cents, 2, Some("USD")),
        crate::usage::fmt_minor(total_cents, 2, Some("USD"))
    );
    let balance = format!("{pair} remaining");
    match credit_expiry_clause(expires_at, now) {
        Some(clause) => format!("{balance} · {clause}"),
        None => balance,
    }
}

/// The grant as the same meter On-Demand uses: remaining dollars beside a bar
/// of how much of the grant is already spent, with that spend in the caption.
///
/// `(used percent, remaining dollars, caption)`. The percent is half-up, the
/// same rounding as a usage bar, and it stays within 0–100 because a grant
/// cannot be spent past its own total.
pub fn cursor_credit_meter(
    remaining_cents: i64,
    total_cents: i64,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> (u16, String, String) {
    let total = total_cents.max(0);
    let remaining = remaining_cents.max(0).min(total);
    let spent = total.saturating_sub(remaining);
    let pct = if total <= 0 {
        0
    } else {
        let rounded = (i128::from(spent) * 100 + i128::from(total) / 2) / i128::from(total);
        u16::try_from(rounded).unwrap_or(100).min(100)
    };
    let mut caption = format!(
        "{} of {} used ({pct}%)",
        crate::usage::fmt_minor(spent, 2, Some("USD")),
        crate::usage::fmt_minor(total, 2, Some("USD"))
    );
    if let Some(clause) = credit_expiry_clause(expires_at, now) {
        caption.push_str(" · ");
        caption.push_str(&clause);
    }
    (
        pct,
        crate::usage::fmt_minor(remaining, 2, Some("USD")),
        caption,
    )
}

fn credit_expiry_clause(expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<String> {
    match expires_at {
        Some(expires) if expires <= now => Some(format!("expired {}", local_date_hm(expires))),
        Some(expires) => Some(format!(
            "expires {} ({})",
            local_date_hm(expires),
            crate::countdown::format(Some(expires), now)
        )),
        None => None,
    }
}

/// The title Cursor puts on the grant.
///
/// The spending card is "Credits". The IDE banner says "Use your $N free
/// credits", or names a scoped product ("Cursor Grok 4.6 Credit", "Cloud
/// Agent Credits", "Bugbot Credit"). `display_name` is that product title
/// when it says "credit". A server label that does not ("Power user grant")
/// and a `grant_type` such as "promo" are not the words on those screens, so
/// the row says "Credits".
pub fn cursor_credit_label(display_name: &str) -> String {
    let name = display_name.trim();
    if name.to_lowercase().contains("credit") {
        name.to_string()
    } else {
        "Credits".to_string()
    }
}

pub fn updated_at_hm(now: DateTime<Utc>, cache_age: Option<Duration>) -> String {
    match cache_age {
        Some(age) => local_time_hm(now - chrono::Duration::from_std(age).unwrap_or_default()),
        None => "—".to_string(),
    }
}

/// Substitute every `{key}` in `template` with `values[key]`. Unknown keys
/// are left as-is.
///
/// This is a single-pass scan; an O(N) implementation that does no
/// re-substitution. (Avoids the bash pitfall where replacement text
/// containing `{foo}` would get further substituted.)
pub fn substitute(template: &str, values: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while !rest.is_empty() {
        match rest.find('{') {
            None => {
                out.push_str(rest);
                break;
            }
            Some(open) => {
                // Copy everything up to the '{'.
                out.push_str(&rest[..open]);
                let after_open = &rest[open + 1..];
                if let Some(close) = after_open.find('}') {
                    let key = &after_open[..close];
                    if let Some(val) = values.get(key) {
                        out.push_str(val);
                        rest = &after_open[close + 1..];
                        continue;
                    }
                }
                // Unmatched or unknown — keep the '{' literal and continue.
                out.push('{');
                rest = after_open;
            }
        }
    }
    out
}

/// Convenience: build a placeholder map from `(&str, impl Into<String>)` pairs.
pub fn placeholders<I, V>(pairs: I) -> HashMap<&'static str, String>
where
    I: IntoIterator<Item = (&'static str, V)>,
    V: Into<String>,
{
    pairs.into_iter().map(|(k, v)| (k, v.into())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        placeholders(pairs.iter().map(|(k, v)| (*k, v.to_string())))
    }

    /// The doc example above is not a CI gate — `.github/workflows/ci.yml` runs
    /// `cargo test --all-targets`, which skips doctests — so the rule every
    /// gauge in the app clamps through is asserted here as well.
    #[test]
    fn a_grant_is_titled_the_way_cursor_titles_it() {
        assert_eq!(cursor_credit_label(""), "Credits");
        assert_eq!(cursor_credit_label("Power user grant"), "Credits");
        assert_eq!(cursor_credit_label("promo"), "Credits");
        assert_eq!(
            cursor_credit_label("Cursor Grok 4.6 Credit"),
            "Cursor Grok 4.6 Credit"
        );
        assert_eq!(
            cursor_credit_label("Cloud Agent Credits"),
            "Cloud Agent Credits"
        );
    }

    #[test]
    fn a_spent_grant_stays_a_full_meter() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let (pct, value, caption) = cursor_credit_meter(0, 2500, None, now);
        assert_eq!(pct, 100);
        assert_eq!(value, "$0.00");
        assert_eq!(caption, "$25.00 of $25.00 used (100%)");
    }

    #[test]
    fn a_percentage_is_rounded_held_to_the_meter_and_nan_safe() {
        assert_eq!(clamp_pct(0.0), 0);
        assert_eq!(clamp_pct(100.0), 100);
        assert_eq!(clamp_pct(24.5), 25);
        assert_eq!(clamp_pct(24.4), 24);
        // Out of range in either direction stops at the end of the meter
        // rather than drawing past it or wrapping.
        assert_eq!(clamp_pct(-0.4), 0);
        assert_eq!(clamp_pct(-9_999.0), 0);
        assert_eq!(clamp_pct(140.0), 100);
        assert_eq!(clamp_pct(f64::INFINITY), 100);
        assert_eq!(clamp_pct(f64::NEG_INFINITY), 0);
        // A denominator off the wire can be zero, and `0.0 / 0.0` is NaN. That
        // is "nothing to draw", not a panic and not a cast to garbage.
        assert_eq!(clamp_pct(f64::NAN), 0);
    }

    fn offer(title: Option<&str>, expires: &str) -> ResetCredit {
        ResetCredit {
            title: title.map(str::to_string),
            expires_at: Some(expires.parse().unwrap()),
        }
    }

    #[test]
    fn a_reset_inventory_lists_each_credit_soonest_first() {
        let now = "2026-07-04T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let credits = ResetCredits {
            available: 2,
            credits: vec![
                offer(Some("Full reset (Weekly + 5 hr)"), "2026-08-01T00:00:00Z"),
                offer(Some("Full reset (Weekly + 5 hr)"), "2026-07-17T00:00:00Z"),
            ],
        };
        assert_eq!(reset_credits(&credits), "2 resets available");
        let lines = reset_credit_lines(&credits, now);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("Full reset (Weekly + 5 hr)"), "{lines:?}");
        assert!(lines[0].contains("(13d 0h)"), "{lines:?}");
        assert!(lines[1].contains("(28d 0h)"), "{lines:?}");
    }

    #[test]
    fn a_lapsed_deadline_reads_as_expired_rather_than_now() {
        let now = "2026-07-04T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let lines = reset_credit_lines(
            &ResetCredits {
                available: 1,
                credits: vec![offer(None, "2026-07-01T00:00:00Z")],
            },
            now,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("Expired "), "{lines:?}");
        assert!(!lines[0].contains("(now)"), "{lines:?}");
    }

    #[test]
    fn a_count_without_detail_still_renders_the_inventory() {
        let now = "2026-07-04T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            reset_credit_lines(
                &ResetCredits {
                    available: 2,
                    credits: vec![]
                },
                now
            ),
            vec!["2 resets available"]
        );
    }

    #[test]
    fn single_substitution() {
        let v = pm(&[("session_pct", "42")]);
        assert_eq!(substitute("{session_pct}%", &v), "42%");
    }

    #[test]
    fn usd_keeps_the_sign_outside_the_symbol() {
        assert_eq!(usd(74.5), "$74.50");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(-5.71), "-$5.71");
        // A negative zero off the wire is not a debt, and must not print as
        // one — `format!("{:.2}")` alone would render it "$-0.00".
        assert_eq!(usd(-0.0), "$0.00");
        // Neither is a debt too small to show a cent.
        assert_eq!(usd(-0.001), "$0.00");
        // One that does round to a cent keeps its sign.
        assert_eq!(usd(-0.006), "-$0.01");
    }

    #[test]
    fn money_places_the_sign_ahead_of_every_currency() {
        assert_eq!(money(20.0, "CNY"), "¥20.00");
        assert_eq!(money(-20.0, "CNY"), "-¥20.00");
        // A currency trails its code only when this table has no symbol for
        // it; the sign still leads either way.
        assert_eq!(money(3.5, "SEK"), "3.50 SEK");
        assert_eq!(money(-3.5, "SEK"), "-3.50 SEK");
        // usd() is the same policy, not a second one.
        assert_eq!(money(-5.71, "USD"), usd(-5.71));
        assert_eq!(money(-0.0, "CNY"), "¥0.00");
    }

    /// `money` and `usage::fmt_minor` compute different numbers — f64 at two
    /// decimals versus integer minor units at the currency's own scale — but a
    /// euro must look like a euro in both. They disagreed once: `money` had no
    /// EUR/GBP/BRL/JPY entry and trailed the code while `fmt_minor` printed the
    /// symbol, so the same currency read two ways in two panels.
    #[test]
    fn the_two_money_formatters_agree_on_every_symbol() {
        for (code, symbol) in [
            ("USD", "$"),
            ("BRL", "R$"),
            ("EUR", "€"),
            ("GBP", "£"),
            ("JPY", "¥"),
            ("CNY", "¥"),
        ] {
            assert_eq!(money(3.5, code), format!("{symbol}3.50"), "money {code}");
            assert_eq!(
                crate::usage::fmt_minor(350, 2, Some(code)),
                format!("{symbol}3.50"),
                "fmt_minor {code}"
            );
        }
        // And they agree that an unlisted code trails instead of guessing.
        assert_eq!(money(3.5, "SEK"), "3.50 SEK");
        assert_eq!(crate::usage::fmt_minor(350, 2, Some("SEK")), "3.50 SEK");
        // fmt_minor keeps its own scale: JPY has no minor unit.
        assert_eq!(crate::usage::fmt_minor(350, 0, Some("JPY")), "¥350");
    }

    #[test]
    fn multiple_substitutions() {
        let v = pm(&[("a", "1"), ("b", "2")]);
        assert_eq!(substitute("{a}-{b}-{a}", &v), "1-2-1");
    }

    #[test]
    fn tokens_compact_every_allowance_scale() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1K");
        assert_eq!(tokens(12_400), "12.4K");
        assert_eq!(tokens(812_400_000), "812.4M");
        assert_eq!(tokens(2_500_000_000), "2.5B");
        assert_eq!(tokens(625_000_000), "625M");
        assert_eq!(tokens(-2_500), "-2.5K");
    }

    #[test]
    fn unknown_placeholder_passes_through() {
        let v = pm(&[("a", "1")]);
        assert_eq!(substitute("{a} {unknown}", &v), "1 {unknown}");
    }

    #[test]
    fn no_re_substitution_in_replacement_text() {
        // Replacement text containing {a} must NOT be re-expanded.
        let v = pm(&[("a", "{a}"), ("b", "X")]);
        assert_eq!(substitute("{b}{a}{b}", &v), "X{a}X");
    }

    #[test]
    fn empty_template() {
        let v = pm(&[("a", "1")]);
        assert_eq!(substitute("", &v), "");
    }

    #[test]
    fn template_without_braces() {
        let v = pm(&[("a", "1")]);
        assert_eq!(substitute("hello world", &v), "hello world");
    }

    #[test]
    fn unmatched_open_brace_is_literal() {
        let v = pm(&[("a", "1")]);
        assert_eq!(substitute("{a {x", &v), "{a {x");
    }

    #[test]
    fn placeholders_with_underscores_and_digits() {
        let v = pm(&[("session_pct_2", "x")]);
        assert_eq!(substitute("{session_pct_2}", &v), "x");
    }

    #[test]
    fn utf8_around_braces() {
        let v = pm(&[("x", "→")]);
        assert_eq!(substitute("α{x}β", &v), "α→β");
    }
}
