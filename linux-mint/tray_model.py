"""Presentation rules for the Linux Mint tray, independent of GTK."""

from datetime import datetime, timezone


def metric_display(metric):
    """Follow the upstream `headline` contract: it selects a field, not text."""
    percent = metric.get("percent")
    if metric.get("headline") == "value" and metric.get("value") not in (None, ""):
        return str(metric["value"])
    if isinstance(percent, (int, float)):
        return f"{percent:g}%"
    return str(metric.get("value") or "Não informado")


def is_not_connected(entry):
    error = str(entry.get("error") or "").lower()
    return entry.get("status") == "error" and (
        "credentials error" in error or "no api key" in error or "not signed in" in error
    )


def connection_help(entry):
    error = str(entry.get("error") or "").lower()
    if "no api key" in error:
        return ("Este provedor precisa de uma chave de API. Configure a chave em "
                "~/.config/ai-usagebar/config.toml ou na variável de ambiente indicada "
                "pelo guia de autenticação.")
    if "not signed in" in error:
        return "Faça login no CLI oficial deste provedor ou configure uma chave de API compatível."
    return "A autenticação deste provedor ainda não está disponível. Consulte o guia de autenticação."


def compact_duration(seconds):
    minutes = max(1, int(seconds // 60))
    days, minutes = divmod(minutes, 1440)
    hours, minutes = divmod(minutes, 60)
    if days:
        return f"{days}d {hours}h"
    if hours:
        return f"{hours}h {minutes}m"
    return f"{minutes}m"


def metric_pace(metric, now=None):
    """Project usage at reset from the same fields used by the upstream tray."""
    used = metric.get("percent")
    window = metric.get("window_secs")
    if not isinstance(used, (int, float)) or not isinstance(window, (int, float)):
        return None
    if used <= 0 or window <= 0:
        return None
    try:
        reset = datetime.fromisoformat(str(metric.get("reset_at") or "").replace("Z", "+00:00"))
        now = now or datetime.now(timezone.utc)
        remaining = (reset - now).total_seconds()
    except (ValueError, TypeError):
        return None
    if not 0 < remaining <= window:
        return None
    elapsed = window - remaining
    # Keep the official popover's 1% warmup, bounded from one minute to one hour.
    if elapsed < min(3600, max(60, window * 0.01)):
        return None
    projected = used * window / elapsed
    if projected <= 90:
        return ("ahead", None)
    if projected <= 100:
        return ("near", f"~{max(0, round(100 - projected))}% de folga")
    if used >= 100:
        return ("behind", "🔥 Limite atingido")
    until_limit = (100 - used) * elapsed / used
    if 0 < until_limit < remaining:
        return ("behind", f"🔥 Limite em {compact_duration(until_limit)}")
    return None


def meter_color(metric, pace):
    if isinstance(metric.get("percent"), (int, float)) and metric["percent"] >= 100:
        return "red"
    if pace:
        return {"ahead": "blue", "near": "yellow", "behind": "red"}[pace[0]]
    return {"critical": "red", "high": "yellow", "mid": "yellow"}.get(
        metric.get("severity"), "blue"
    )
