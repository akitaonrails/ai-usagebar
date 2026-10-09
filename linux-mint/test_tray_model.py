"""Hermetic contracts for the Mint tray's report presentation."""

import unittest
import json
import tempfile
from pathlib import Path
from datetime import datetime, timedelta, timezone

from tray_model import connection_help, installed_binary, is_not_connected, meter_color, metric_display, metric_pace, metric_usage_label, report_sections, summary_metric, tr


NOW = datetime(2026, 9, 25, 12, 0, tzinfo=timezone.utc)


def metric(used, window=18_000, elapsed=3_600):
    return {
        "percent": used,
        "window_secs": window,
        "reset_at": (NOW + timedelta(seconds=window - elapsed)).isoformat(),
    }


class TrayModelTest(unittest.TestCase):
    def test_grokbot_report_projects_the_actual_period(self):
        fixture = json.loads((Path(__file__).resolve().parent.parent / "tests/fixtures/grokbot_paced_report.json").read_text())
        entry = fixture["entries"][0]
        row = list(report_sections(entry))[0]
        projected = metric_pace(row, NOW, "en_US")
        self.assertEqual(projected, ("behind", "🔥 Limit in 2d 3h"))
        self.assertEqual(meter_color(row, projected), "red")
        self.assertEqual(metric_pace({**row, "window_secs": None}, NOW, "en_US"), None)

    def test_cursor_report_paces_each_pool_against_the_billing_cycle(self):
        fixture = json.loads((Path(__file__).resolve().parent.parent / "tests/fixtures/cursor_paced_report.json").read_text())
        ahead, under = list(report_sections(fixture["entries"][0]))
        self.assertEqual(metric_pace(ahead, NOW, "en_US"), ("behind", "🔥 Limit in 2d 3h"))
        self.assertEqual(metric_pace(under, NOW, "en_US"), ("ahead", None))
        self.assertEqual(metric_pace({**ahead, "window_secs": None}, NOW, "en_US"), None)

    def test_binary_resolution_supports_cargo_and_saved_override(self):
        with tempfile.TemporaryDirectory() as temporary_home:
            home = Path(temporary_home)
            cargo = home / ".cargo/bin/ai-usagebar"
            cargo.parent.mkdir(parents=True)
            cargo.write_text("#!/bin/sh\n")
            cargo.chmod(0o755)
            self.assertEqual(installed_binary("ai-usagebar", str(home)), str(cargo))

            custom = home / "custom location/ai-usagebar"
            custom.parent.mkdir()
            custom.write_text("#!/bin/sh\n")
            custom.chmod(0o755)
            config = home / ".local/share/ai-usagebar/tray/binaries.json"
            config.parent.mkdir(parents=True)
            config.write_text(json.dumps({"ai-usagebar": str(custom)}))
            self.assertEqual(installed_binary("ai-usagebar", str(home)), str(custom))
            self.assertEqual(installed_binary("ai-usagebar", str(home), str(cargo)), str(cargo))

    def test_projection_sets_color_and_flame_with_a_tolerance(self):
        ahead = metric_pace(metric(10), NOW, "pt_BR")
        near = metric_pace(metric(19), NOW, "pt_BR")
        over = metric_pace(metric(24), NOW, "pt_BR")
        behind = metric_pace(metric(30), NOW, "pt_BR")
        self.assertEqual(ahead, ("ahead", None))
        self.assertEqual(near, ("near", "~5% de folga"))
        self.assertEqual(over, ("over", "~20% acima do ritmo"))
        self.assertEqual(behind, ("behind", "🔥 Limite em 2h 20m"))
        self.assertEqual(meter_color(metric(10), ahead), "blue")
        self.assertEqual(meter_color(metric(19), near), "blue")
        self.assertEqual(meter_color(metric(24), over), "yellow")
        self.assertEqual(meter_color(metric(30), behind), "red")

    def test_a_hair_over_the_line_is_calm_not_a_run_out(self):
        # A hair over the line is inside the tolerance: no flame, blue.
        pace = metric_pace(metric(21), NOW, "en_US")
        self.assertEqual(pace, ("near", "~0% spare"))
        self.assertEqual(meter_color(metric(21), pace), "blue")

    def test_early_in_a_week_the_gap_keeps_rounding_calm(self):
        week, eight_hours = 604_800, 28_800
        # 7% used 8h into a week projects ~147% but sits ~2 points past the tick.
        calm = metric_pace(metric(7, week, eight_hours), NOW, "en_US")
        self.assertEqual(calm[0], "near")
        self.assertEqual(meter_color(metric(7, week, eight_hours), calm), "blue")
        self.assertEqual(metric_pace(metric(9, week, eight_hours), NOW)[0], "over")
        self.assertEqual(metric_pace(metric(10, week, eight_hours), NOW)[0], "over")
        self.assertEqual(metric_pace(metric(55, week, eight_hours), NOW)[0], "behind")

    def test_weekly_warmup_is_capped_at_one_hour(self):
        self.assertIsNone(metric_pace(metric(1, 604_800, 3_599), NOW))
        self.assertIsNotNone(metric_pace(metric(1, 604_800, 3_600), NOW))

    def test_without_pace_meter_color_reads_what_is_left(self):
        for used, color in ((0, "blue"), (2, "blue"), (50, "blue"), (51, "yellow"),
                            (59, "yellow"), (80, "yellow"), (81, "red"), (100, "red")):
            self.assertEqual(meter_color({"percent": used}), color, used)
        self.assertEqual(meter_color({}), "blue")
        self.assertEqual(meter_color({"percent": "n/a"}), "blue")

    def test_missing_or_expired_reset_has_no_pace(self):
        row = metric(40)
        row["reset_at"] = "not a date"
        self.assertIsNone(metric_pace(row, NOW))
        row["reset_at"] = (NOW - timedelta(seconds=1)).isoformat()
        self.assertIsNone(metric_pace(row, NOW))

    def test_headline_and_disconnected_error_contract(self):
        self.assertEqual(metric_display({"headline": "value", "value": "12 credits", "percent": 40}), "12 credits")
        self.assertEqual(metric_display({"percent": 40}), "40%")
        self.assertTrue(is_not_connected({"status": "error", "error": "credentials error: OpenRouter: no API key"}))
        self.assertFalse(is_not_connected({"status": "error", "error": "HTTP 500"}))
        self.assertFalse(is_not_connected({"status": "error", "error": "credentials error: expired", "stale": True,
                                           "metrics": [{"label": "Weekly", "percent": 45}]}))

    def test_report_sections_preserve_order_and_metric_groups(self):
        entry = {"sections": [
            {"type": "text", "label": "Account", "value": "Pro"},
            {"type": "metric", "label": "Overall", "percent": 10},
            {"type": "metric", "label": "Daily", "group": "Breakdown", "percent": 20},
            {"type": "metric", "label": "Weekly", "group": "Breakdown", "percent": 30},
            {"type": "block", "label": "Sessions", "body": ["one session"]},
        ]}
        self.assertEqual(
            [(row["type"], row.get("label")) for row in report_sections(entry)],
            [("text", "Account"), ("metric", "Overall"),
             ("heading", "Breakdown"), ("metric", "Daily"),
             ("metric", "Weekly"), ("block", "Sessions")],
        )

    def test_menu_summary_reads_quota_windows_not_context_sessions(self):
        metrics = [
            {"label": "Session (5h)", "percent": 29},
            {"label": "Weekly", "percent": 35},
            {"label": "ship the release", "percent": 90, "group": "Sessions"},
        ]
        self.assertEqual(summary_metric({"metrics": metrics})["label"], "Weekly")
        # A grouped row still stands in when the entry has nothing else.
        self.assertEqual(summary_metric({"metrics": metrics[2:]})["percent"], 90)
        self.assertIsNone(summary_metric({"metrics": []}))

    def test_expired_antigravity_session_explains_remote_refresh_setup(self):
        entry = {"status": "error", "error": "Credentials error: Antigravity's saved Google session expired and ai-usagebar has no OAuth client to refresh it"}
        self.assertTrue(is_not_connected(entry))
        help_text = connection_help(entry, "pt_BR")
        self.assertIn("oauth_client_id", help_text)
        self.assertIn("aplicativo fechado", help_text)

    def test_english_is_default_and_portuguese_is_selected_explicitly(self):
        self.assertEqual(tr("Settings", "Configurações", "en_US"), "Settings")
        self.assertEqual(tr("Settings", "Configurações", "pt_BR"), "Configurações")
        self.assertEqual(metric_pace(metric(19), NOW, "en_US"), ("near", "~5% spare"))
        self.assertEqual(metric_usage_label({"headline": "value", "value": "$12", "percent": 40}, "en_US"), "40% used")
        self.assertEqual(metric_usage_label({"headline": "percent", "percent": 40}, "pt_BR"), "40% usado")


if __name__ == "__main__":
    unittest.main()
