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

pub fn local_time_hm(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local).format("%H:%M").to_string()
}

pub fn local_time_hms(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub fn updated_at_hm(now: DateTime<Utc>, cache_age: Option<Duration>) -> String {
    match cache_age {
        Some(age) => local_time_hm(now - chrono::Duration::from_std(age).unwrap_or_default()),
        None => "—".to_string(),
    }
}

pub fn updated_at_hms(now: DateTime<Utc>, cache_age: Option<Duration>) -> String {
    match cache_age {
        Some(age) => local_time_hms(now - chrono::Duration::from_std(age).unwrap_or_default()),
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

    #[test]
    fn single_substitution() {
        let v = pm(&[("session_pct", "42")]);
        assert_eq!(substitute("{session_pct}%", &v), "42%");
    }

    #[test]
    fn multiple_substitutions() {
        let v = pm(&[("a", "1"), ("b", "2")]);
        assert_eq!(substitute("{a}-{b}-{a}", &v), "1-2-1");
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
