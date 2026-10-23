//! Quota priority with a bounded, local history of fresh observations.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub const DEFAULT_RECENT_HOURS: u8 = 3;
pub const MAX_RECENT_HOURS: u8 = 24;
const HISTORY_SECONDS: i64 = (MAX_RECENT_HOURS as i64 + 1) * 3600;
const MAX_SAMPLES: usize = 16_384;
const MIN_UNUSED_PERCENT: f64 = 5.0;
const DAY_SECONDS: f64 = 86_400.0;
// A cached reading (served during a backoff) still describes a weekly quota
// well for a few hours; older than this it is treated as unavailable.
const MAX_STALE_SECONDS: i64 = 6 * 3600;
const WEEK_SECONDS: f64 = 7.0 * DAY_SECONDS;
pub const DEFAULT_ALERT_UNUSED_PERCENT: u8 = 50;
pub const DEFAULT_ALERT_HOURS: u8 = 24;
pub const MAX_ALERT_HOURS: u8 = 168;

/// When a weekly quota counts as urgent: at least `unused_percent` projected
/// unused and a reset within `hours`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alert {
    pub unused_percent: u8,
    pub hours: u8,
}

impl Default for Alert {
    fn default() -> Self {
        Self {
            unused_percent: DEFAULT_ALERT_UNUSED_PERCENT,
            hours: DEFAULT_ALERT_HOURS,
        }
    }
}

/// What `usage --json --priorities` was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub recent_hours: u8,
    pub alert: Alert,
}

impl Alert {
    fn clamped(self) -> Self {
        Self {
            unused_percent: self.unused_percent.clamp(1, 100),
            hours: self.hours.clamp(1, MAX_ALERT_HOURS),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Sample {
    entry: String,
    label: String,
    at: i64,
    reset: i64,
    percent: f64,
}

fn timestamp(v: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.timestamp())
}

/// Tests inject storage. No credentials or conversations are recorded.
pub fn update_history(path: &Path, entries: &[Value], now: i64) -> std::io::Result<Vec<Sample>> {
    std::fs::create_dir_all(path)?;
    let _lock = crate::cache::acquire_lock(&path.join("history.lock"), Duration::from_secs(1))
        .map_err(std::io::Error::other)?;
    let file = path.join("history.json");
    let mut samples: Vec<Sample> = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    samples.retain(|s| s.at >= now - HISTORY_SECONDS && s.at <= now);
    for e in entries {
        if e["status"] != "ready" {
            continue;
        }
        let Some(at) = timestamp(&e["fetched_at"]).filter(|t| *t <= now && now - *t <= 1800) else {
            continue;
        };
        for m in e["metrics"].as_array().into_iter().flatten() {
            let (Some(percent), Some(reset), Some(id), Some(label)) = (
                m["percent"].as_f64(),
                timestamp(&m["reset_at"]),
                e["id"].as_str(),
                m["label"].as_str(),
            ) else {
                continue;
            };
            if m["headline"] != "percent" || !(0.0..=100.0).contains(&percent) {
                continue;
            }
            if samples
                .iter()
                .any(|s| s.entry == id && s.label == label && s.at == at && s.reset == reset)
            {
                continue;
            }
            samples.push(Sample {
                entry: id.into(),
                label: label.into(),
                at,
                reset,
                percent,
            });
        }
    }
    samples.sort_by_key(|s| s.at);
    if samples.len() > MAX_SAMPLES {
        samples.drain(..samples.len() - MAX_SAMPLES);
    }
    crate::cache::atomic_write(&file, &serde_json::to_vec(&samples)?)
        .map_err(std::io::Error::other)?;
    Ok(samples)
}

fn recent_rate(
    samples: &[Sample],
    entry: &Value,
    metric: &Value,
    now: i64,
    recent_hours: u8,
) -> Option<(f64, f64)> {
    let reset = timestamp(&metric["reset_at"])?;
    let at = timestamp(&entry["fetched_at"])?;
    if now - at > 1800 || at > now {
        return None;
    }
    let used = metric["percent"].as_f64()?;
    let mut chain: Vec<&Sample> = Vec::new();
    for s in samples.iter().filter(|s| {
        Some(s.entry.as_str()) == entry["id"].as_str()
            && Some(s.label.as_str()) == metric["label"].as_str()
            && (s.reset - reset).abs() <= 120
            && s.at >= at - i64::from(recent_hours.clamp(1, MAX_RECENT_HOURS)) * 3600
            && s.at <= at
    }) {
        if chain
            .last()
            .is_some_and(|last| s.percent < last.percent || s.at - last.at > 1800)
        {
            chain.clear();
        }
        chain.push(s);
    }
    let last = chain.last()?;
    let first = chain.first()?;
    // A decrease means a reset/correction: don't turn it into negative consumption.
    if at - last.at > 1800 || used < last.percent || at - first.at < 1800 {
        return None;
    }
    let hours = (at - first.at) as f64 / 3600.0;
    Some(((used - first.percent) / hours, hours))
}

#[cfg(test)]
fn rank(entries: &[Value], samples: &[Sample], now: DateTime<Utc>) -> Vec<Value> {
    rank_with_recent_hours(
        entries,
        samples,
        now,
        DEFAULT_RECENT_HOURS,
        Alert::default(),
    )
}

pub fn rank_with_recent_hours(
    entries: &[Value],
    samples: &[Sample],
    now: DateTime<Utc>,
    recent_hours: u8,
    alert: Alert,
) -> Vec<Value> {
    let recent_hours = recent_hours.clamp(1, MAX_RECENT_HOURS);
    let alert = alert.clamped();
    let horizon = f64::from(alert.hours) * 3600.0;
    let mut ranking = Vec::new();
    for e in entries {
        let mut row = json!({"entry_id":e["id"],"name":e["display_name"],"status":"unknown", "reason":"No reset window with a known duration", "score":-1.0, "urgent":false, "recent_window_hours":recent_hours, "reason_code":"unknown"});
        let fetched = timestamp(&e["fetched_at"]);
        let too_old = fetched.is_none_or(|t| now.timestamp() - t > MAX_STALE_SECONDS);
        if e["status"] != "ready" || (e["stale"] == true && too_old) {
            row["status"] = json!("unavailable");
            row["reason_code"] = json!("unavailable");
            row["reason"] = json!("Usage data is unavailable or outdated");
            ranking.push(row);
            continue;
        }
        let metrics = e["metrics"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let shared = |m: &&Value| {
            m["headline"] == "percent"
                && m["model_pool"] != true
                && m.get("group").is_none_or(Value::is_null)
        };
        let session_exhausted = metrics.iter().filter(shared).any(|m| {
            m["window_secs"]
                .as_f64()
                .is_some_and(|w| w < WEEK_SECONDS - DAY_SECONDS)
                && m["percent"].as_f64().unwrap_or(0.0) >= 100.0
        });
        // Quota is lost at the weekly reset, so the weekly window is the
        // measure; a session/rolling window refills within hours. With several
        // weekly windows (Claude's shared week and a per-model week) the most
        // used one binds what can still be spent. Providers without a weekly
        // window fall back to their longest known window of at least a day.
        let windowed: Vec<(&Value, f64, i64, f64)> = metrics
            .iter()
            .filter(shared)
            .filter_map(|m| {
                Some((
                    m,
                    m["window_secs"].as_f64().filter(|w| *w >= DAY_SECONDS)?,
                    timestamp(&m["reset_at"])?,
                    m["percent"].as_f64()?,
                ))
            })
            .collect();
        let weekly = |w: f64| (w - WEEK_SECONDS).abs() <= DAY_SECONDS;
        let pool: Vec<_> = if windowed.iter().any(|c| weekly(c.1)) {
            windowed.into_iter().filter(|c| weekly(c.1)).collect()
        } else {
            let longest = windowed.iter().map(|c| c.1).fold(0.0, f64::max);
            windowed.into_iter().filter(|c| c.1 == longest).collect()
        };
        // `max_by` keeps the last of equals; reversing keeps the first listed.
        let Some(&(m, window, reset, used)) = pool
            .iter()
            .rev()
            .max_by(|a, b| a.3.total_cmp(&b.3).then(b.2.cmp(&a.2)))
        else {
            ranking.push(row);
            continue;
        };
        row["window"] = m["label"].clone();
        row["reset_at"] = m["reset_at"].clone();
        if used >= 100.0 {
            row["status"] = json!("blocked");
            row["reason_code"] = json!("blocked");
            row["reason"] = json!("The weekly usage window is exhausted");
            ranking.push(row);
            continue;
        }
        let remaining = (reset - now.timestamp()) as f64;
        if remaining <= 0.0 || window <= 0.0 || used < 0.0 {
            ranking.push(row);
            continue;
        }
        let remaining_h = remaining / 3600.0;
        // Usage follows a daily rhythm: the first hours of a cycle are averaged
        // over a whole day instead of being extrapolated around the clock.
        let elapsed = (window - remaining).max(0.0);
        let cycle = used / (elapsed.max(DAY_SECONDS) / 3600.0);
        let recent = recent_rate(samples, e, m, now.timestamp(), recent_hours);
        let weight = recent
            .map(|(_, h)| (h / f64::from(recent_hours) * 0.75).min(0.75))
            .unwrap_or(0.0);
        let blended = recent
            .map(|(r, _)| r * weight + cycle * (1.0 - weight))
            .unwrap_or(cycle);
        // The recent pace speaks for the next day; the cycle average for the rest.
        let near = remaining_h.min(24.0);
        let consumed = blended * near + cycle * (remaining_h - near);
        let left = 100.0 - used;
        let unused = (left - consumed).clamp(0.0, left);
        let score = unused / remaining_h;
        let urgent = remaining <= horizon
            && unused >= f64::from(alert.unused_percent).max(MIN_UNUSED_PERCENT);
        let priority = unused >= MIN_UNUSED_PERCENT;
        row = json!({"entry_id":e["id"],"name":e["display_name"],"window":m["label"],"reset_at":m["reset_at"],
            "unused_percent":unused.round(),"remaining_percent":left.round(),"required_percent_per_hour":left/remaining_h,
            "pace_percent_per_hour":consumed/remaining_h,"cycle_percent_per_hour":cycle,"recent_percent_per_hour":recent.map(|r|r.0),
            "recent_hours":recent.map(|r|r.1),"recent_weight":weight,"basis":if recent.is_some(){"recent_and_cycle"}else{"cycle_average"},
            "session_exhausted":session_exhausted, "stale":e["stale"] == true, "fetched_at":e["fetched_at"],
            "urgent":urgent, "alert_unused_percent":alert.unused_percent, "alert_hours":alert.hours, "recent_window_hours":recent_hours,
            "reason_code":if priority{"priority"}else{"on_track"},"score":score,"status":if priority{"priority"}else{"on_track"},
            "reason":if priority{"Quota is projected to reset with unused capacity"}else{"The estimated pace already uses the quota before reset"}});
        ranking.push(row);
    }
    ranking.sort_by(|a, b| {
        let tier = |v: &Value| match v["status"].as_str().unwrap_or("") {
            "priority" if v["urgent"] == true => 0,
            "priority" => 1,
            "on_track" => 2,
            "unknown" => 3,
            "blocked" => 4,
            _ => 5,
        };
        tier(a)
            .cmp(&tier(b))
            .then_with(|| {
                b["score"]
                    .as_f64()
                    .unwrap_or(-1.0)
                    .total_cmp(&a["score"].as_f64().unwrap_or(-1.0))
            })
            .then_with(|| a["entry_id"].as_str().cmp(&b["entry_id"].as_str()))
    });
    ranking
}

/// Keep disk I/O and the inter-process lock off the async report's runtime.
pub async fn attach_history(document: String, options: Options) -> String {
    let Options {
        recent_hours,
        alert,
    } = options;
    let fallback = document.clone();
    tokio::task::spawn_blocking(move || {
        let Ok(mut doc) = serde_json::from_str::<Value>(&document) else {
            return document;
        };
        let Some(entries) = doc["entries"].as_array() else {
            return document;
        };
        let recent_hours = recent_hours.clamp(1, MAX_RECENT_HOURS);
        let alert = alert.clamped();
        let now = Utc::now();
        let history = crate::cache::xdg_cache_dir().ok().and_then(|p| {
            update_history(&p.join("ai-usagebar/priority"), entries, now.timestamp()).ok()
        });
        let ranking = rank_with_recent_hours(
            entries,
            history.as_deref().unwrap_or(&[]),
            now,
            recent_hours,
            alert,
        );
        doc["recommendation"] = ranking
            .iter()
            .find(|r| r["status"] == "priority")
            .cloned()
            .unwrap_or(Value::Null);
        doc["ranking"] = json!(ranking);
        doc["priority_history_available"] = json!(history.is_some());
        doc["priority_recent_hours"] = json!(recent_hours);
        doc["priority_alert"] =
            json!({"unused_percent": alert.unused_percent, "hours": alert.hours});
        doc.to_string()
    })
    .await
    .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1800000000;
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(NOW, 0).unwrap()
    }
    fn entry(id: &str, used: u16) -> Value {
        json!({"id":id,"display_name":id,"status":"ready","stale":false,"fetched_at":now().to_rfc3339(),"metrics":[{"label":"Weekly","headline":"percent","percent":used,"window_secs":604800,"reset_at":DateTime::from_timestamp(NOW+86400,0).unwrap().to_rfc3339()}]})
    }
    fn sample(at: i64, p: f64) -> Sample {
        Sample {
            entry: "a".into(),
            label: "Weekly".into(),
            at,
            reset: NOW + 86400,
            percent: p,
        }
    }
    #[test]
    fn recent_acceleration_changes_order() {
        let a = entry("a", 20);
        let b = entry("b", 30);
        assert_eq!(
            rank(&[a.clone(), b.clone()], &[], now())[0]["entry_id"],
            "a"
        );
        let samples: Vec<_> = (0..=6)
            .map(|i| sample(NOW - 10800 + i * 1800, i as f64 * 20.0 / 6.0))
            .collect();
        let rows = rank(&[a, b], &samples, now());
        assert_eq!(rows[0]["entry_id"], "b");
        assert_eq!(rows[1]["basis"], "recent_and_cycle");
    }
    #[test]
    fn reset_and_gap_do_not_create_a_recent_rate() {
        let e = entry("a", 20);
        let m = &e["metrics"][0];
        assert!(
            recent_rate(
                &[sample(NOW - 3600, 30.0), sample(NOW, 20.0)],
                &e,
                m,
                NOW,
                DEFAULT_RECENT_HOURS
            )
            .is_none()
        );
        assert!(
            recent_rate(
                &[sample(NOW - 10800, 0.0), sample(NOW, 20.0)],
                &e,
                m,
                NOW,
                DEFAULT_RECENT_HOURS
            )
            .is_none()
        );
        let mut wrong = sample(NOW - 3600, 0.0);
        wrong.reset -= 604800;
        assert!(
            recent_rate(
                &[wrong, sample(NOW, 20.0)],
                &e,
                m,
                NOW,
                DEFAULT_RECENT_HOURS
            )
            .is_none()
        );
    }
    #[test]
    fn all_entries_remain_visible() {
        let mut stale = entry("stale", 20);
        stale["stale"] = json!(true);
        stale["fetched_at"] = json!(
            DateTime::from_timestamp(NOW - MAX_STALE_SECONDS - 1, 0)
                .unwrap()
                .to_rfc3339()
        );
        let mut unknown = entry("unknown", 20);
        unknown["metrics"][0]["window_secs"] = Value::Null;
        let r = rank(
            &[
                entry("a", 20),
                entry("blocked", 100),
                stale,
                unknown,
                entry("ontrack", 99),
            ],
            &[],
            now(),
        );
        assert_eq!(r.len(), 5);
        assert_eq!(r[0]["status"], "priority");
        assert_eq!(r[1]["status"], "on_track");
        assert_eq!(r[4]["status"], "unavailable");
    }
    #[test]
    fn history_deduplicates_fetches_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry("a", 20);
        assert_eq!(
            update_history(dir.path(), std::slice::from_ref(&e), NOW)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            update_history(dir.path(), &[e], NOW + 300).unwrap().len(),
            1
        );
        assert!(
            update_history(dir.path(), &[], NOW + HISTORY_SECONDS + 1)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn zero_recent_usage_is_measured_not_missing() {
        let e = entry("a", 20);
        let samples: Vec<_> = (0..=6)
            .map(|i| sample(NOW - 10800 + i * 1800, 20.0))
            .collect();
        let r = rank(&[e], &samples, now());
        assert_eq!(r[0]["recent_percent_per_hour"], 0.0);
        assert_eq!(r[0]["recent_weight"], 0.75);
    }
    #[test]
    fn configured_window_changes_the_rate_and_priority() {
        // Four hours of inactivity, followed by one hour of fast consumption.
        let samples: Vec<_> = (0..=10)
            .map(|i| {
                sample(
                    NOW - 18000 + i * 1800,
                    if i <= 8 { 0.0 } else { (i - 8) as f64 * 10.0 },
                )
            })
            .collect();
        let entries = [entry("a", 20), entry("b", 75)];
        let short = rank_with_recent_hours(&entries, &samples, now(), 1, Alert::default());
        let long = rank_with_recent_hours(&entries, &samples, now(), 6, Alert::default());
        assert_eq!(short[0]["entry_id"], "b");
        assert_eq!(long[0]["entry_id"], "a");
        assert_eq!(short[1]["recent_percent_per_hour"], 20.0);
        assert_eq!(long[0]["recent_percent_per_hour"], 4.0);
        assert_eq!(long[0]["recent_window_hours"], 6);
    }
    #[test]
    fn urgency_requires_a_real_unused_quota_before_a_near_reset() {
        let mut tomorrow = entry("tomorrow", 20);
        tomorrow["metrics"][0]["reset_at"] = json!(
            DateTime::from_timestamp(NOW + 86401, 0)
                .unwrap()
                .to_rfc3339()
        );
        let rows = rank(
            &[
                entry("urgent", 20),
                entry("ontrack", 99),
                entry("blocked", 100),
                tomorrow,
            ],
            &[],
            now(),
        );
        assert_eq!(rows[0]["entry_id"], "urgent");
        assert!(
            rows.iter()
                .filter(|r| r["entry_id"] != "urgent")
                .all(|r| r["urgent"] == false)
        );
    }
    #[test]
    fn short_windows_are_never_urgent_and_do_not_mask_a_weekly_reset() {
        let reset = |secs: i64| {
            DateTime::from_timestamp(NOW + secs, 0)
                .unwrap()
                .to_rfc3339()
        };
        let mut session = entry("session", 10);
        session["metrics"][0]["window_secs"] = json!(18000);
        session["metrics"][0]["reset_at"] = json!(reset(3600));
        assert_eq!(rank(&[session], &[], now())[0]["urgent"], false);

        let mut both = entry("both", 30);
        both["metrics"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"label":"Session (5h)","headline":"percent","percent":5,"window_secs":18000,"reset_at":reset(3600)}));
        let row = &rank(&[both], &[], now())[0];
        assert_eq!(row["urgent"], true);
        assert_eq!(row["window"], "Weekly");
    }
    fn metric(label: &str, percent: f64, window: Option<i64>, reset_in: i64) -> Value {
        json!({"label":label,"headline":"percent","percent":percent,"window_secs":window,
            "reset_at":DateTime::from_timestamp(NOW+reset_in,0).unwrap().to_rfc3339()})
    }
    fn provider(id: &str, metrics: Vec<Value>) -> Value {
        json!({"id":id,"display_name":id,"status":"ready","stale":false,"fetched_at":now().to_rfc3339(),"metrics":metrics})
    }
    #[test]
    fn the_binding_weekly_window_is_the_measure() {
        // Claude: session, shared week and a lighter per-model week.
        let claude = provider(
            "anthropic",
            vec![
                metric("Session (5h)", 8.0, Some(18000), 3600),
                metric("Weekly (7d)", 31.0, Some(604800), 23 * 3600),
                metric("Fable (7d)", 4.0, Some(604800), 23 * 3600),
            ],
        );
        let row = &rank(&[claude], &[], now())[0];
        assert_eq!(row["window"], "Weekly (7d)");
        assert_eq!(row["remaining_percent"], 69.0);
        let unused = row["unused_percent"].as_f64().unwrap();
        assert!(
            unused > 0.0 && unused <= 69.0,
            "never more than what is left: {unused}"
        );
    }
    #[test]
    fn early_cycle_usage_is_not_extrapolated_around_the_clock() {
        // 2% in the first two hours of a week is not 1%/h for seven days.
        let codex = provider(
            "openai",
            vec![metric("Codex weekly", 2.0, Some(604800), 604800 - 7200)],
        );
        let row = &rank(&[codex], &[], now())[0];
        assert_eq!(row["status"], "priority");
        assert!(row["unused_percent"].as_f64().unwrap() > 80.0);
    }
    #[test]
    fn a_fresh_untouched_week_is_a_priority_not_unknown() {
        // Antigravity repeats pool labels per window and starts a week at 0%.
        let agy = provider(
            "antigravity",
            vec![
                metric("Gemini", 0.0, Some(18000), 18000),
                metric("Gemini", 0.0, Some(604800), 4 * 86400),
            ],
        );
        let row = &rank(std::slice::from_ref(&agy), &[], now())[0];
        assert_eq!(row["status"], "priority");
        assert_eq!(row["window"], "Gemini", "an even tie keeps the first pool");
        assert_eq!(row["unused_percent"], 100.0);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(update_history(dir.path(), &[agy], NOW).unwrap().len(), 2);
    }
    #[test]
    fn a_recent_cached_reading_is_still_ranked() {
        let mut cached = entry("cached", 20);
        cached["stale"] = json!(true);
        let row = &rank(std::slice::from_ref(&cached), &[], now())[0];
        assert_eq!(row["status"], "priority");
        assert_eq!(row["stale"], true);
        cached["fetched_at"] = json!(
            DateTime::from_timestamp(NOW - MAX_STALE_SECONDS - 1, 0)
                .unwrap()
                .to_rfc3339()
        );
        assert_eq!(rank(&[cached], &[], now())[0]["status"], "unavailable");
    }
    #[test]
    fn session_limits_inform_but_do_not_block_the_week() {
        let p = provider(
            "a",
            vec![
                metric("Session (5h)", 100.0, Some(18000), 3600),
                metric("Weekly", 40.0, Some(604800), 3 * 86400),
            ],
        );
        let row = &rank(&[p], &[], now())[0];
        assert_eq!(row["status"], "priority");
        assert_eq!(row["session_exhausted"], true);
        let spent = provider("b", vec![metric("Weekly", 100.0, Some(604800), 86400)]);
        assert_eq!(rank(&[spent], &[], now())[0]["status"], "blocked");
    }
    #[test]
    fn without_a_week_the_longest_known_window_is_used() {
        let p = provider(
            "a",
            vec![
                metric("Rolling (5h)", 10.0, Some(18000), 3600),
                metric("Monthly", 20.0, Some(30 * 86400), 20 * 86400),
                metric("Unknown", 50.0, None, 86400),
            ],
        );
        assert_eq!(rank(&[p], &[], now())[0]["window"], "Monthly");
        let session_only = provider("s", vec![metric("Session", 10.0, Some(18000), 3600)]);
        assert_eq!(rank(&[session_only], &[], now())[0]["status"], "unknown");
    }
    #[test]
    fn alert_thresholds_are_configurable() {
        // 60% used, resets in 24h: ~37% projected unused at the cycle pace.
        let rows = |unused_percent, hours| {
            rank_with_recent_hours(
                &[entry("a", 60)],
                &[],
                now(),
                DEFAULT_RECENT_HOURS,
                Alert {
                    unused_percent,
                    hours,
                },
            )[0]["urgent"]
                .clone()
        };
        assert_eq!(rows(50, 24), false);
        assert_eq!(rows(30, 24), true);
        assert_eq!(rows(30, 12), false, "reset is outside a 12h horizon");
        // The weekly window is the measure, so a full-week horizon still alerts.
        assert_eq!(rows(30, 168), true);
    }
    #[test]
    fn history_retains_a_full_day_without_counting_old_fetches_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = entry("a", 20);
        old["fetched_at"] = json!(
            DateTime::from_timestamp(NOW - 86400, 0)
                .unwrap()
                .to_rfc3339()
        );
        update_history(dir.path(), &[old], NOW - 86400).unwrap();
        assert_eq!(update_history(dir.path(), &[], NOW).unwrap().len(), 1);
        assert!(
            update_history(dir.path(), &[], NOW + 3601)
                .unwrap()
                .is_empty()
        );
    }
}
