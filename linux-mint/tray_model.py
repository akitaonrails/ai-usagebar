"""Presentation rules for the Linux Mint tray, independent of GTK."""

import os
import json
from datetime import datetime, timezone


def installed_binary(name, home=None, override=None):
    """Resolve only explicit or fixed install locations, never ambient PATH."""
    home = home or os.path.expanduser("~")
    if override:
        return override
    config_path = os.path.join(home, ".local/share/ai-usagebar/tray/binaries.json")
    try:
        with open(config_path, encoding="utf-8") as config_file:
            configured = json.load(config_file).get(name)
        if configured and os.path.isabs(configured) and os.access(configured, os.X_OK):
            return configured
    except (OSError, ValueError, TypeError, AttributeError):
        pass
    for base in (os.path.join(home, ".local/bin"), os.path.join(home, ".cargo/bin"),
                 "/usr/local/bin", "/usr/bin"):
        candidate = os.path.join(base, name)
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return os.path.join(home, ".local/bin", name)


def tr(english, portuguese, language=None):
    if language is None:
        language = (os.environ.get("LC_ALL") or os.environ.get("LC_MESSAGES")
                    or os.environ.get("LANG") or "en")
    return portuguese if language.lower().startswith("pt") else english


def metric_display(metric):
    """Follow the upstream `headline` contract: it selects a field, not text."""
    percent = metric.get("percent")
    if metric.get("headline") == "value" and metric.get("value") not in (None, ""):
        return str(metric["value"])
    if isinstance(percent, (int, float)):
        return f"{percent:g}%"
    return str(metric.get("value") or tr("Not available", "Não informado"))


def metric_usage_label(metric, language=None):
    percent = metric.get("percent")
    if not isinstance(percent, (int, float)):
        return None
    value = f"{percent:g}%" if metric.get("headline") == "value" else metric_display(metric)
    return tr(f"{value} used", f"{value} usado", language)


def is_not_connected(entry):
    error = str(entry.get("error") or "").lower()
    has_snapshot = bool(entry.get("sections") or entry.get("metrics"))
    return not has_snapshot and entry.get("status") == "error" and (
        "credentials error" in error or "no api key" in error or "not signed in" in error
    )


def connection_help(entry, language=None):
    error = str(entry.get("error") or "").lower()
    if "oauth client" in error and "antigravity" in error:
        return tr("The saved Antigravity session expired. To read quota while the app is closed, "
                  "set the public oauth_client_id and oauth_client_secret under [antigravity] "
                  "in config.toml. The login token remains in Antigravity's own storage.",
                  "A sessão salva do Antigravity expirou. Para consultar a cota com o aplicativo fechado, "
                  "configure oauth_client_id e oauth_client_secret públicos em [antigravity] no "
                  "config.toml; o token de login continua no armazenamento do próprio Antigravity.", language)
    if "no api key" in error:
        return tr("This provider needs an API key. Set it in ~/.config/ai-usagebar/config.toml "
                  "or in the environment variable named by the authentication guide.",
                  "Este provedor precisa de uma chave de API. Configure a chave em "
                  "~/.config/ai-usagebar/config.toml ou na variável de ambiente indicada "
                  "pelo guia de autenticação.", language)
    if "not signed in" in error:
        return tr("Sign in with this provider's official CLI or configure a compatible API key.",
                  "Faça login no CLI oficial deste provedor ou configure uma chave de API compatível.", language)
    return tr("Authentication is unavailable for this provider. See the authentication guide.",
              "A autenticação deste provedor ainda não está disponível. Consulte o guia de autenticação.", language)


def report_sections(entry):
    """Keep the report order and expose metric subgroups as headings."""
    sections = entry.get("sections") or [
        {"type": "metric", **metric} for metric in entry.get("metrics", [])
    ]
    previous_group = None
    for section in sections:
        if not isinstance(section, dict):
            continue
        group = section.get("group") if section.get("type") == "metric" else None
        if group and group != previous_group:
            yield {"type": "heading", "label": group}
        yield section
        previous_group = group


def compact_duration(seconds):
    minutes = max(1, int(seconds // 60))
    days, minutes = divmod(minutes, 1440)
    hours, minutes = divmod(minutes, 60)
    if days:
        return f"{days}d {hours}h"
    if hours:
        return f"{hours}h {minutes}m"
    return f"{minutes}m"


def metric_pace(metric, now=None, language=None):
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
        spare = max(0, round(100 - projected))
        return ("near", tr(f"~{spare}% spare", f"~{spare}% de folga", language))
    if used >= 100:
        return ("behind", tr("🔥 Limit reached", "🔥 Limite atingido", language))
    until_limit = (100 - used) * elapsed / used
    if 0 < until_limit < remaining:
        duration = compact_duration(until_limit)
        return ("behind", tr(f"🔥 Limit in {duration}", f"🔥 Limite em {duration}", language))
    return None


def meter_color(metric, pace):
    if isinstance(metric.get("percent"), (int, float)) and metric["percent"] >= 100:
        return "red"
    if pace:
        return {"ahead": "blue", "near": "yellow", "behind": "red"}[pace[0]]
    return {"critical": "red", "high": "yellow", "mid": "yellow"}.get(
        metric.get("severity"), "blue"
    )
