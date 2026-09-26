"""Hermetic contracts for the Mint tray's report presentation."""

import unittest
import json
import tempfile
from pathlib import Path
from datetime import datetime, timedelta, timezone

from tray_model import connection_help, installed_binary, is_not_connected, meter_color, metric_display, metric_pace, metric_usage_label, report_sections, tr


NOW = datetime(2026, 9, 25, 12, 0, tzinfo=timezone.utc)


def metric(used, window=18_000, elapsed=3_600):
    return {
        "percent": used,
        "window_secs": window,
        "reset_at": (NOW + timedelta(seconds=window - elapsed)).isoformat(),
    }


class TrayModelTest(unittest.TestCase):
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

    def test_projection_controls_color_and_flame(self):
        ahead = metric_pace(metric(10), NOW, "pt_BR")
        near = metric_pace(metric(19), NOW, "pt_BR")
        behind = metric_pace(metric(30), NOW, "pt_BR")
        self.assertEqual(ahead, ("ahead", None))
        self.assertEqual(near, ("near", "~5% de folga"))
        self.assertEqual(behind, ("behind", "🔥 Limite em 2h 20m"))
        self.assertEqual(meter_color(metric(10), ahead), "blue")
        self.assertEqual(meter_color(metric(19), near), "yellow")
        self.assertEqual(meter_color(metric(30), behind), "red")

    def test_weekly_warmup_is_capped_at_one_hour(self):
        self.assertIsNone(metric_pace(metric(1, 604_800, 3_599), NOW))
        self.assertIsNotNone(metric_pace(metric(1, 604_800, 3_600), NOW))

    def test_missing_or_expired_reset_uses_severity_fallback(self):
        row = metric(40)
        row["reset_at"] = "not a date"
        row["severity"] = "high"
        self.assertIsNone(metric_pace(row, NOW))
        self.assertEqual(meter_color(row, None), "yellow")
        row["reset_at"] = (NOW - timedelta(seconds=1)).isoformat()
        self.assertIsNone(metric_pace(row, NOW))
        self.assertEqual(meter_color({"percent": 100}, None), "red")

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
