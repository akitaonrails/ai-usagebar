//! Source-compatibility checks for renderer helpers used by library consumers.

use ai_usagebar::minimax::vendor as minimax_vendor;
use ai_usagebar::theme::Theme;
use ai_usagebar::tooltip::{self, WindowRow};
use ai_usagebar::usage::{MinimaxSnapshot, UsageWindow, ZaiSnapshot};
use ai_usagebar::zai::vendor as zai_vendor;
use chrono::{TimeZone, Utc};

#[test]
fn pace_placeholder_builders_keep_their_two_argument_api() {
    let now = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
    let window = UsageWindow {
        utilization_pct: 25,
        resets_at: Some(now + chrono::Duration::hours(1)),
        window_duration: chrono::Duration::hours(5),
    };

    let zai = ZaiSnapshot {
        plan: "GLM Coding Pro".into(),
        session: Some(window.clone()),
        weekly: None,
        mcp: None,
    };
    assert_eq!(
        zai_vendor::build_placeholders(&zai, now)["session_pct"],
        "25"
    );

    let minimax = MinimaxSnapshot {
        plan: "MiniMax Token Plan".into(),
        session: window.clone(),
        weekly: window,
        video_session: None,
        video_weekly: None,
    };
    assert_eq!(
        minimax_vendor::build_placeholders(&minimax, now)["session_pct"],
        "25"
    );
}

/// The macOS menu bar reads Z.AI's MCP pool through these three placeholders in
/// its `--format` string (`macos/ai-usagebar-menubar.swift`, fields 31-33).
/// Renaming one here would silently drop that row, so pin the names — and pin
/// the `—` an absent pool reports, which is the presence signal the Swift
/// parser keys off to avoid painting a phantom 0% row.
#[test]
fn the_zai_mcp_placeholders_the_macos_menu_bar_reads_keep_their_names() {
    let now = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
    let mcp = UsageWindow {
        utilization_pct: 7,
        resets_at: Some(now + chrono::Duration::days(24)),
        window_duration: chrono::Duration::days(30),
    };
    let with_mcp = ZaiSnapshot {
        plan: "GLM Coding Pro".into(),
        session: None,
        weekly: None,
        mcp: Some(mcp),
    };
    let v = zai_vendor::build_placeholders(&with_mcp, now);
    assert_eq!(v["zai_mcp_pct"], "7");
    assert_eq!(v["zai_mcp_reset"], "24d 0h");
    assert_eq!(v["zai_mcp_elapsed"], "20");

    let without_mcp = ZaiSnapshot {
        mcp: None,
        ..with_mcp
    };
    let v = zai_vendor::build_placeholders(&without_mcp, now);
    assert_eq!(
        v["zai_mcp_reset"], "—",
        "an absent pool must stay distinguishable from a real 0% one"
    );
}

/// The macOS menu bar reads Model Studio's monthly Token Plan pool through
/// these placeholders in its `--format` string (`macos/ai-usagebar-menubar.swift`,
/// fields 63-64). Renaming one here would silently drop that row, so pin the
/// names and pin the empty string an absent pool reports (the missing-placeholder
/// convention the Swift parser keys off to avoid painting a phantom 0% row).
#[test]
fn the_modelstudio_monthly_placeholders_the_macos_menu_bar_reads_keep_their_names() {
    let now = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
    let monthly = UsageWindow {
        utilization_pct: 42,
        resets_at: Some(now + chrono::Duration::days(18)),
        window_duration: chrono::Duration::days(30),
    };
    let with_monthly = ai_usagebar::usage::ModelStudioSnapshot {
        session: None,
        weekly: None,
        monthly: Some(monthly),
    };
    let opts = ai_usagebar::vendor::RenderOpts {
        format: None,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
    };
    let v = ai_usagebar::modelstudio::vendor::build_placeholders(&with_monthly, &opts, now);
    assert_eq!(v.get("mst_monthly_pct").map(String::as_str), Some("42"));
    assert_eq!(
        v.get("mst_monthly_reset").map(String::as_str),
        Some("18d 0h")
    );

    let unknown_reset = ai_usagebar::usage::ModelStudioSnapshot {
        session: None,
        weekly: None,
        monthly: Some(UsageWindow {
            utilization_pct: 35,
            resets_at: None,
            window_duration: chrono::Duration::days(30),
        }),
    };
    let v = ai_usagebar::modelstudio::vendor::build_placeholders(&unknown_reset, &opts, now);
    assert_eq!(v.get("mst_monthly_pct").map(String::as_str), Some("35"));
    assert_eq!(v.get("mst_monthly_reset").map(String::as_str), Some("—"));

    let without_monthly = ai_usagebar::usage::ModelStudioSnapshot {
        session: None,
        weekly: None,
        monthly: None,
    };
    let v = ai_usagebar::modelstudio::vendor::build_placeholders(&without_monthly, &opts, now);
    assert_eq!(
        v.get("mst_monthly_pct").map(String::as_str),
        Some(""),
        "an absent monthly window must yield an empty string so the macOS parser skips it"
    );
    assert_eq!(v.get("mst_monthly_reset").map(String::as_str), Some(""));
}

/// The macOS menu bar reads Synthetic's subscription quota through these
/// placeholders in its `--format` string (`macos/ai-usagebar-menubar.swift`,
/// fields 65-66). Renaming one here would silently drop that row, so pin the
/// names and pin placeholder values.
#[test]
fn the_synthetic_placeholders_the_macos_menu_bar_reads_keep_their_names() {
    let now = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
    let subscription = UsageWindow {
        utilization_pct: 60,
        resets_at: Some(now + chrono::Duration::days(15)),
        window_duration: chrono::Duration::days(30),
    };
    let snapshot = ai_usagebar::usage::SyntheticSnapshot {
        requests: 600.0,
        limit: 1000.0,
        subscription,
    };
    let opts = ai_usagebar::vendor::RenderOpts {
        format: None,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
    };
    let v = ai_usagebar::synthetic::vendor::build_placeholders(&snapshot, &opts, now);
    assert_eq!(v.get("synthetic_pct").map(String::as_str), Some("60"));
    assert_eq!(v.get("synthetic_reset").map(String::as_str), Some("15d 0h"));
    assert_eq!(
        v.get("session_pct").map(String::as_str),
        Some("60"),
        "the generic session alias must remain populated for backward compatibility"
    );
    assert_eq!(v.get("session_reset").map(String::as_str), Some("15d 0h"));
}

/// `push_window` is the shared tooltip row helper. Its sixth argument stayed an
/// `Option<i32>` marker when the row gained a pace glyph: that lives on
/// `WindowRow` behind `push_window_with_row`, so a library caller written
/// against the original six-argument form still compiles.
#[test]
fn push_window_keeps_its_option_marker_api() {
    let now = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
    let window = UsageWindow {
        utilization_pct: 40,
        resets_at: Some(now + chrono::Duration::hours(3)),
        window_duration: chrono::Duration::hours(5),
    };
    let theme = Theme::default();

    let mut plain = Vec::new();
    tooltip::push_window(&mut plain, "  L", &window, &theme, now, None);
    let plain = tooltip::render_bordered(&plain, &theme);

    // The delegate must be indistinguishable from the row form it forwards to.
    let mut forwarded = Vec::new();
    tooltip::push_window_with_row(
        &mut forwarded,
        "  L",
        &window,
        &theme,
        now,
        WindowRow::default(),
    );
    assert_eq!(plain, tooltip::render_bordered(&forwarded, &theme));

    // And it must not have grown a glyph behind its callers' backs.
    for glyph in ['\u{2191}', '\u{2192}', '\u{2193}'] {
        assert!(!plain.contains(glyph), "{plain}");
    }

    // The marker argument still reaches the bar.
    let mut marked = Vec::new();
    tooltip::push_window(&mut marked, "  L", &window, &theme, now, Some(40));
    assert_ne!(plain, tooltip::render_bordered(&marked, &theme));
}
