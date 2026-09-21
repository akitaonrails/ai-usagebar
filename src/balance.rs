//! Prepaid-balance display policy: which denominator a tank is measured
//! against, how much of it is consumed, and which number goes on the bar.
//!
//! A prepaid vendor's API reports money *remaining* and nothing else — there is
//! no denominator to turn that into a meter. `[vendor] display_limit` lets the
//! user state the tank size themselves, in the currency that vendor already
//! reports. It is a fallback, never an override: a vendor that states a limit of
//! its own (OpenRouter's credits purchased, or a per-key limit) keeps it.
//!
//! Which of the two numbers a frontend puts on the bar is a separate choice.
//! [`Headline`] is the user's preference; [`MetricHeadline`] is what a report
//! metric actually declares after the preference meets the available data.
//!
//! Who acts on it, as of this commit:
//! - The Omarchy panel and the KDE plasmoid read the declaration and draw the
//!   number it names, instead of guessing from the row's label.
//! - The Windows tray popover also consumes `usage --json`, but its
//!   `normalizeSection` drops the field, so it always draws the percentage. A
//!   `display_limit` still reaches it — the row it receives turns from a text
//!   balance into a meter — but `headline = "amount"` does not.
//! - Waybar, GNOME and the macOS menu bar are fed by the per-vendor
//!   `{placeholder}` formats rather than by report sections, so neither setting
//!   reaches them at all.

use serde::{Deserialize, Serialize};

/// Which number a vendor puts on the bar.
///
/// A balance vendor defaults to [`Headline::Amount`], a quota vendor to
/// [`Headline::Percent`]. Setting `display_limit` does not change this by
/// itself — the tank size and the headline are independent choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Headline {
    /// The money figure — a balance, in the vendor's currency.
    Amount,
    /// The consumed percentage of the tank. Falls back to the amount when
    /// nothing supplies a denominator.
    Percent,
}

/// The headline a report metric declares, after [`Headline`] has met the data.
///
/// Frontends draw this number and leave the other one in the detail line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricHeadline {
    /// Draw `percent`.
    Percent,
    /// Draw `value` (the money figure).
    Value,
}

impl MetricHeadline {
    /// The word this headline serializes as in the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Percent => "percent",
            Self::Value => "value",
        }
    }
}

/// Per-vendor bar-number settings, resolved from config for one tab.
///
/// The default is the quota-vendor shape — no user denominator, percent on the
/// bar — so a vendor that never opts in is unaffected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayPrefs {
    /// `[vendor] display_limit`: the user's tank size, if they stated one.
    pub display_limit: Option<f64>,
    /// `[vendor] headline`.
    pub headline: Headline,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        Self {
            display_limit: None,
            headline: Headline::Percent,
        }
    }
}

impl DisplayPrefs {
    /// Prefs for a vendor whose API states no limit of its own.
    pub fn balance(display_limit: Option<f64>, headline: Headline) -> Self {
        Self {
            display_limit,
            headline,
        }
    }
}

/// A usable tank size, or `None`.
///
/// The API's own limit wins; `display_limit` is the fallback; without either
/// there is no denominator and no meter can be drawn. A value that cannot
/// divide — zero, negative, infinite, NaN — counts as absent at either
/// position, so a bad API response degrades to the user's number rather than
/// producing a nonsense percentage.
///
/// # Examples
///
/// ```
/// use ai_usagebar::balance::denominator;
///
/// assert_eq!(denominator(Some(50.0), Some(200.0)), Some(50.0));
/// assert_eq!(denominator(None, Some(200.0)), Some(200.0));
/// assert_eq!(denominator(None, None), None);
/// ```
pub fn denominator(api_limit: Option<f64>, display_limit: Option<f64>) -> Option<f64> {
    api_limit
        .filter(usable)
        .or_else(|| display_limit.filter(usable))
}

fn usable(limit: &f64) -> bool {
    limit.is_finite() && *limit > 0.0
}

/// Whole percent of `limit` already consumed, given how much is left.
///
/// Consumed rather than remaining, so the meter fills the way the quota meters
/// do, and clamped through [`crate::format::clamp_pct`]: a balance above the cap
/// reads as 0% used (the money figure still says how far above), and an
/// overdrawn balance stops at 100%.
///
/// # Examples
///
/// ```
/// use ai_usagebar::balance::consumed_pct;
///
/// assert_eq!(consumed_pct(200.0, 50.0), 75);
/// assert_eq!(consumed_pct(200.0, 250.0), 0);
/// assert_eq!(consumed_pct(200.0, -10.0), 100);
/// ```
pub fn consumed_pct(limit: f64, remaining: f64) -> u16 {
    if !usable(&limit) {
        return 0;
    }
    crate::format::clamp_pct((limit - remaining) / limit * 100.0)
}

/// Which number this metric puts on the bar.
///
/// `percent` needs a denominator; asked for one without a limit from either
/// source, the amount stays on the bar rather than a fabricated percentage.
pub fn resolve_headline(choice: Headline, denominator: Option<f64>) -> MetricHeadline {
    match choice {
        Headline::Amount => MetricHeadline::Value,
        Headline::Percent if denominator.is_some() => MetricHeadline::Percent,
        Headline::Percent => MetricHeadline::Value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the fallback order ---

    #[test]
    fn an_api_limit_wins_over_the_users_display_limit() {
        assert_eq!(denominator(Some(50.0), Some(200.0)), Some(50.0));
    }

    #[test]
    fn display_limit_is_used_only_when_the_api_states_no_limit() {
        assert_eq!(denominator(None, Some(200.0)), Some(200.0));
        assert_eq!(denominator(Some(50.0), None), Some(50.0));
        assert_eq!(denominator(None, None), None);
    }

    /// No baked-in default: absent means absent, at both positions.
    #[test]
    fn an_unusable_limit_counts_as_absent_at_either_position() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(denominator(Some(bad), Some(200.0)), Some(200.0), "{bad}");
            assert_eq!(denominator(None, Some(bad)), None, "{bad}");
            assert_eq!(denominator(Some(bad), None), None, "{bad}");
        }
    }

    // --- the math ---

    #[test]
    fn percent_is_consumed_not_remaining() {
        assert_eq!(consumed_pct(200.0, 200.0), 0);
        assert_eq!(consumed_pct(200.0, 150.0), 25);
        assert_eq!(consumed_pct(200.0, 100.0), 50);
        assert_eq!(consumed_pct(200.0, 0.0), 100);
    }

    /// A balance above the cap is 0% used, not a negative percentage — the
    /// money figure is what says how far above it sits.
    #[test]
    fn a_balance_above_the_cap_reads_as_zero_percent_used() {
        assert_eq!(consumed_pct(200.0, 250.0), 0);
        assert_eq!(consumed_pct(20.0, 1_000.0), 0);
    }

    #[test]
    fn an_overdrawn_balance_stops_at_one_hundred() {
        assert_eq!(consumed_pct(200.0, -0.01), 100);
        assert_eq!(consumed_pct(200.0, -5_000.0), 100);
    }

    #[test]
    fn percent_rounds_to_the_nearest_whole() {
        // 5.5 / 20 consumed = 27.5% -> 28.
        assert_eq!(consumed_pct(20.0, 14.5), 28);
        // 4.9 / 20 = 24.5% -> 25 (round-half-away-from-zero).
        assert_eq!(consumed_pct(20.0, 15.1), 25);
    }

    #[test]
    fn a_limit_that_cannot_divide_yields_zero_rather_than_a_nonsense_percent() {
        for bad in [0.0, -10.0, f64::NAN, f64::INFINITY] {
            assert_eq!(consumed_pct(bad, 5.0), 0, "{bad}");
        }
        assert_eq!(consumed_pct(200.0, f64::NAN), 0);
    }

    // --- the headline switch ---

    #[test]
    fn amount_keeps_the_money_figure_on_the_bar_even_with_a_tank() {
        assert_eq!(
            resolve_headline(Headline::Amount, Some(200.0)),
            MetricHeadline::Value
        );
        assert_eq!(
            resolve_headline(Headline::Amount, None),
            MetricHeadline::Value
        );
    }

    #[test]
    fn percent_needs_a_denominator_and_otherwise_keeps_the_amount() {
        assert_eq!(
            resolve_headline(Headline::Percent, Some(200.0)),
            MetricHeadline::Percent
        );
        assert_eq!(
            resolve_headline(Headline::Percent, None),
            MetricHeadline::Value
        );
    }

    #[test]
    fn the_report_words_are_percent_and_value() {
        assert_eq!(MetricHeadline::Percent.as_str(), "percent");
        assert_eq!(MetricHeadline::Value.as_str(), "value");
    }

    #[test]
    fn the_default_prefs_are_the_quota_shape() {
        let prefs = DisplayPrefs::default();
        assert_eq!(prefs.display_limit, None);
        assert_eq!(prefs.headline, Headline::Percent);
    }

    #[test]
    fn headline_parses_from_the_config_words_and_rejects_anything_else() {
        #[derive(Deserialize)]
        struct Wrapper {
            headline: Headline,
        }
        let parse = |body: &str| toml::from_str::<Wrapper>(body).map(|w| w.headline);

        assert_eq!(parse("headline = \"amount\"").unwrap(), Headline::Amount);
        assert_eq!(parse("headline = \"percent\"").unwrap(), Headline::Percent);
        // A typo is loud at load time rather than silently drawing the wrong
        // number for the life of the install.
        assert!(parse("headline = \"dollars\"").is_err());
        assert!(parse("headline = \"Amount\"").is_err());
    }
}
