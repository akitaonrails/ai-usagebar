"""Hermetic contracts for the Mint tray's report presentation."""

import unittest
from datetime import datetime, timedelta, timezone

from tray_model import is_not_connected, meter_color, metric_display, metric_pace


NOW = datetime(2026, 9, 25, 12, 0, tzinfo=timezone.utc)


def metric(used, window=18_000, elapsed=3_600):
    return {
        "percent": used,
        "window_secs": window,
        "reset_at": (NOW + timedelta(seconds=window - elapsed)).isoformat(),
    }


class TrayModelTest(unittest.TestCase):
    def test_projection_controls_color_and_flame(self):
        ahead = metric_pace(metric(10), NOW)
        near = metric_pace(metric(19), NOW)
        behind = metric_pace(metric(30), NOW)
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


if __name__ == "__main__":
    unittest.main()
