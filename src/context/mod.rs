//! Best-effort, local-only Claude Code context-window discovery.
//!
//! Claude Code transcripts are an undocumented JSONL surface, so this module
//! deliberately treats every line as optional data. It reads only bounded
//! tails, ignores unknown/corrupt records, never follows discovered symlinks,
//! and reports unknown usage instead of fabricating zero. A compaction record
//! after the latest usage invalidates that reading until another assistant
//! response supplies the new context size.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::config::ContextConfig;
use crate::error::{AppError, Result};

/// The overlay is a recent-session monitor, not an unbounded history browser.
pub const MAX_SESSIONS: usize = 100;
const MAX_WALK_ENTRIES: usize = 10_000;
const MAX_TAIL_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 512 * 1024;
const MAX_DISPLAY_CHARS: usize = 120;

#[derive(Debug, Clone)]
pub struct ContextScan {
    pub sessions: Vec<ContextSession>,
    /// JSONL candidates observed before the recent-session cap was applied.
    pub discovered: usize,
    /// Unreadable entries and malformed/oversized tail records ignored.
    pub skipped: usize,
    /// True when the directory-entry safety cap stopped traversal early.
    pub walk_capped: bool,
}

#[derive(Debug, Clone)]
pub struct ContextSession {
    pub session_id: String,
    pub title: Option<String>,
    pub project: String,
    pub model: Option<String>,
    pub modified_at: DateTime<Utc>,
    pub usage: ContextUsage,
}

impl ContextSession {
    pub fn display_name(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("session {}", short_id(&self.session_id)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextUsage {
    Available {
        /// Matches Claude Code's input-only context percentage: fresh input +
        /// cache creation + cache reads from the most recent API response.
        input_tokens: u64,
        window_tokens: Option<u64>,
        percent: Option<u16>,
    },
    /// A compact boundary landed after the last assistant usage record.
    Compacted,
    /// No usable assistant usage was present in the bounded transcript tail.
    Unknown,
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    modified: SystemTime,
}

#[derive(Debug, Default)]
struct Discovery {
    candidates: Vec<Candidate>,
    skipped: usize,
    walk_capped: bool,
}

#[derive(Debug, Default)]
struct TailData {
    session_id: Option<String>,
    title: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    usage: TailUsage,
    skipped: usize,
}

#[derive(Debug, Default)]
enum TailUsage {
    Tokens(u64),
    Compacted,
    #[default]
    Unknown,
}

/// Resolve Claude Code's conventional local transcript directory without
/// reading it. Kept separate so tests always inject their own root.
pub fn default_projects_path() -> Result<PathBuf> {
    Ok(crate::cache::home_dir()?.join(".claude").join("projects"))
}

/// Scan the most recently modified top-level session transcripts below
/// `root`. Subagent sidechains are excluded, and discovered symlinks are never
/// followed. The caller should run this blocking filesystem work on Tokio's
/// blocking pool.
pub fn scan_dir(root: &Path, config: &ContextConfig) -> Result<ContextScan> {
    let mut discovery = discover(root)?;
    discovery.candidates.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    let discovered = discovery.candidates.len();
    discovery.candidates.truncate(MAX_SESSIONS);

    let mut sessions = Vec::with_capacity(discovery.candidates.len());
    for candidate in discovery.candidates {
        match parse_candidate(root, candidate, config) {
            Ok((session, skipped)) => {
                discovery.skipped += skipped;
                sessions.push(session);
            }
            Err(_) => discovery.skipped += 1,
        }
    }

    Ok(ContextScan {
        sessions,
        discovered,
        skipped: discovery.skipped,
        walk_capped: discovery.walk_capped,
    })
}

fn discover(root: &Path) -> Result<Discovery> {
    // Opening the configured root is the one error that should reach the user;
    // failures inside it are isolated to the affected entry.
    fs::read_dir(root).map_err(|e| AppError::io_at(root, e))?;

    let mut result = Discovery::default();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;

    'walk: while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if visited >= MAX_WALK_ENTRIES {
                result.walk_capped = true;
                break 'walk;
            }
            visited += 1;

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if entry.file_name() != "subagents" {
                    stack.push(entry.path());
                }
                continue;
            }
            if !file_type.is_file()
                || entry.path().extension().and_then(|ext| ext.to_str()) != Some("jsonl")
            {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            result.candidates.push(Candidate {
                path: entry.path(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(result)
}

fn parse_candidate(
    root: &Path,
    candidate: Candidate,
    config: &ContextConfig,
) -> Result<(ContextSession, usize)> {
    let tail = read_tail(&candidate.path)?;
    let parsed = parse_tail(&tail);
    let file_id = candidate
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session");
    let session_id = parsed
        .session_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| sanitize_display(file_id));
    let project = parsed
        .cwd
        .as_deref()
        .and_then(project_from_cwd)
        .or_else(|| project_from_path(root, &candidate.path))
        .unwrap_or_else(|| "unknown project".into());
    let window_tokens = config.window_tokens_for(parsed.model.as_deref());
    let usage = match parsed.usage {
        TailUsage::Tokens(input_tokens) => ContextUsage::Available {
            input_tokens,
            window_tokens,
            percent: window_tokens.map(|window| percent(input_tokens, window)),
        },
        TailUsage::Compacted => ContextUsage::Compacted,
        TailUsage::Unknown => ContextUsage::Unknown,
    };

    Ok((
        ContextSession {
            session_id,
            title: parsed.title.filter(|title| !title.is_empty()),
            project,
            model: parsed.model,
            modified_at: DateTime::<Utc>::from(candidate.modified),
            usage,
        },
        parsed.skipped,
    ))
}

fn read_tail(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|e| AppError::io_at(path, e))?;
    let len = file.metadata().map_err(|e| AppError::io_at(path, e))?.len();
    let read_len = len.min(MAX_TAIL_BYTES as u64);
    let start = len.saturating_sub(read_len);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| AppError::io_at(path, e))?;
    let mut bytes = vec![0; read_len as usize];
    file.read_exact(&mut bytes)
        .map_err(|e| AppError::io_at(path, e))?;

    // The first bytes of a bounded tail may be the middle of a JSON record.
    // Drop that fragment rather than feeding it to the tolerant parser.
    if start > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        } else {
            bytes.clear();
        }
    }
    Ok(bytes)
}

fn parse_tail(bytes: &[u8]) -> TailData {
    let mut out = TailData::default();
    for raw in bytes.split(|byte| *byte == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.is_empty() {
            continue;
        }
        if raw.len() > MAX_LINE_BYTES {
            out.skipped += 1;
            continue;
        }
        let value: Value = match serde_json::from_slice(raw) {
            Ok(value) => value,
            Err(_) => {
                out.skipped += 1;
                continue;
            }
        };

        capture_string(&value, "sessionId", &mut out.session_id);
        capture_string(&value, "cwd", &mut out.cwd);
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                capture_string(&value, "customTitle", &mut out.title);
                capture_string(&value, "title", &mut out.title);
            }
            Some("assistant") => {
                if let Some(model) = value.pointer("/message/model").and_then(Value::as_str) {
                    out.model = Some(sanitize_display(model));
                }
                if let Some(tokens) = input_tokens(&value) {
                    out.usage = TailUsage::Tokens(tokens);
                }
            }
            Some("system")
                if value.get("subtype").and_then(Value::as_str) == Some("compact_boundary") =>
            {
                out.usage = TailUsage::Compacted;
            }
            _ => {}
        }
    }
    out
}

fn capture_string(value: &Value, key: &str, target: &mut Option<String>) {
    if let Some(value) = value.get(key).and_then(Value::as_str) {
        let value = sanitize_display(value);
        if !value.is_empty() {
            *target = Some(value);
        }
    }
}

fn input_tokens(value: &Value) -> Option<u64> {
    let usage = value.pointer("/message/usage")?;
    let input = usage.get("input_tokens")?.as_u64()?;
    let cache_creation = optional_u64(usage, "cache_creation_input_tokens")?;
    let cache_read = optional_u64(usage, "cache_read_input_tokens")?;
    input.checked_add(cache_creation)?.checked_add(cache_read)
}

fn optional_u64(value: &Value, key: &str) -> Option<u64> {
    match value.get(key) {
        None | Some(Value::Null) => Some(0),
        Some(value) => value.as_u64(),
    }
}

fn percent(input_tokens: u64, window_tokens: u64) -> u16 {
    debug_assert!(window_tokens > 0);
    let value = (u128::from(input_tokens) * 100) / u128::from(window_tokens);
    value.min(u128::from(u16::MAX)) as u16
}

fn project_from_cwd(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .map(|name| sanitize_display(&name.to_string_lossy()))
        .filter(|name| !name.is_empty())
}

fn project_from_path(root: &Path, transcript: &Path) -> Option<String> {
    let relative = transcript.strip_prefix(root).ok()?;
    let first = relative.components().next()?.as_os_str();
    let project = sanitize_display(&first.to_string_lossy());
    (!project.is_empty()).then_some(project)
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Strip terminal control characters and cap untrusted transcript labels
/// before they enter ratatui's backing buffer.
pub fn sanitize_display(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| {
            if ch.is_control() {
                if ch.is_whitespace() { Some(' ') } else { None }
            } else {
                Some(ch)
            }
        })
        .take(MAX_DISPLAY_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn assistant(
        session: &str,
        cwd: &str,
        model: &str,
        input: u64,
        create: u64,
        read: u64,
    ) -> String {
        json!({
            "type": "assistant",
            "sessionId": session,
            "cwd": cwd,
            "message": {
                "model": model,
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": create,
                    "cache_read_input_tokens": read
                }
            }
        })
        .to_string()
    }

    fn write_session(root: &Path, project: &str, name: &str, lines: &[String]) -> PathBuf {
        let dir = root.join(project);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.jsonl"));
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn latest_assistant_usage_matches_claude_input_only_formula() {
        let dir = TempDir::new().unwrap();
        write_session(
            dir.path(),
            "-work-project",
            "abc-123",
            &[
                assistant("abc-123", "/work/project", "claude-test", 1, 2, 3),
                assistant("abc-123", "/work/project", "claude-test", 10, 20, 30),
            ],
        );
        let mut config = ContextConfig {
            context_window_tokens: Some(200),
            ..ContextConfig::default()
        };
        config
            .model_context_window_tokens
            .insert("claude-test".into(), 100);

        let scan = scan_dir(dir.path(), &config).unwrap();
        assert_eq!(scan.sessions.len(), 1);
        let session = &scan.sessions[0];
        assert_eq!(session.session_id, "abc-123");
        assert_eq!(session.project, "project");
        assert_eq!(session.model.as_deref(), Some("claude-test"));
        assert_eq!(
            session.usage,
            ContextUsage::Available {
                input_tokens: 60,
                window_tokens: Some(100),
                percent: Some(60),
            }
        );
    }

    #[test]
    fn compact_boundary_invalidates_the_previous_reading() {
        let dir = TempDir::new().unwrap();
        let compact = json!({"type": "system", "subtype": "compact_boundary"}).to_string();
        write_session(
            dir.path(),
            "project",
            "session",
            &[
                assistant("session", "/work/project", "claude-test", 90, 0, 0),
                compact,
            ],
        );

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(scan.sessions[0].usage, ContextUsage::Compacted);
    }

    #[test]
    fn assistant_after_compaction_supplies_the_new_reading() {
        let dir = TempDir::new().unwrap();
        let compact = json!({"type": "system", "subtype": "compact_boundary"}).to_string();
        write_session(
            dir.path(),
            "project",
            "session",
            &[
                assistant("session", "/work/project", "claude-test", 90, 0, 0),
                compact,
                assistant("session", "/work/project", "claude-test", 12, 0, 0),
            ],
        );

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(
            scan.sessions[0].usage,
            ContextUsage::Available {
                input_tokens: 12,
                window_tokens: None,
                percent: None,
            }
        );
    }

    #[test]
    fn corrupt_or_truncated_records_do_not_hide_the_last_good_usage() {
        let dir = TempDir::new().unwrap();
        let path = write_session(
            dir.path(),
            "project",
            "session",
            &[assistant(
                "session",
                "/work/project",
                "claude-test",
                42,
                0,
                0,
            )],
        );
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        write!(file, "{{\"type\":\"assistant\"").unwrap();

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert!(scan.skipped >= 1);
        assert_eq!(
            scan.sessions[0].usage,
            ContextUsage::Available {
                input_tokens: 42,
                window_tokens: None,
                percent: None,
            }
        );
    }

    #[test]
    fn an_old_usage_record_outside_the_tail_cap_is_not_loaded_unboundedly() {
        let dir = TempDir::new().unwrap();
        let path = write_session(
            dir.path(),
            "project",
            "session",
            &[assistant(
                "session",
                "/work/project",
                "claude-test",
                42,
                0,
                0,
            )],
        );
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(&vec![b'x'; MAX_TAIL_BYTES + 1024]).unwrap();
        writeln!(file).unwrap();

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(scan.sessions[0].usage, ContextUsage::Unknown);
    }

    #[test]
    fn subagent_transcripts_are_excluded() {
        let dir = TempDir::new().unwrap();
        write_session(
            dir.path(),
            "project",
            "main",
            &[assistant("main", "/work/project", "claude-test", 1, 0, 0)],
        );
        write_session(
            &dir.path().join("project"),
            "subagents",
            "agent",
            &[assistant("agent", "/work/project", "claude-test", 99, 0, 0)],
        );

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(scan.sessions[0].session_id, "main");
    }

    #[cfg(unix)]
    #[test]
    fn discovered_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = write_session(
            outside.path(),
            "project",
            "outside",
            &[assistant("outside", "/outside", "claude-test", 99, 0, 0)],
        );
        fs::create_dir_all(dir.path().join("project")).unwrap();
        symlink(outside_file, dir.path().join("project/linked.jsonl")).unwrap();

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert!(scan.sessions.is_empty());
    }

    #[test]
    fn custom_titles_and_paths_are_sanitized_before_display() {
        let dir = TempDir::new().unwrap();
        let title =
            json!({"type": "custom-title", "customTitle": "build\u{1b}[2J\nrelease"}).to_string();
        write_session(
            dir.path(),
            "project",
            "session",
            &[
                title,
                assistant("session", "/work/proj\nname", "claude-test", 1, 0, 0),
            ],
        );

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        let session = &scan.sessions[0];
        assert_eq!(session.title.as_deref(), Some("build[2J release"));
        assert_eq!(session.project, "proj name");
    }

    #[test]
    fn invalid_usage_numbers_are_unknown_not_zero() {
        let dir = TempDir::new().unwrap();
        let invalid = json!({
            "type": "assistant",
            "message": {"usage": {"input_tokens": "12"}}
        })
        .to_string();
        write_session(dir.path(), "project", "session", &[invalid]);

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(scan.sessions[0].usage, ContextUsage::Unknown);
    }

    #[test]
    fn recent_session_result_is_capped() {
        let dir = TempDir::new().unwrap();
        for i in 0..=MAX_SESSIONS {
            write_session(
                dir.path(),
                "project",
                &format!("session-{i}"),
                &[assistant(
                    &format!("session-{i}"),
                    "/work/project",
                    "claude-test",
                    i as u64,
                    0,
                    0,
                )],
            );
        }

        let scan = scan_dir(dir.path(), &ContextConfig::default()).unwrap();
        assert_eq!(scan.discovered, MAX_SESSIONS + 1);
        assert_eq!(scan.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn missing_root_is_an_actionable_error() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing");
        let error = scan_dir(&missing, &ContextConfig::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains(missing.to_string_lossy().as_ref()));
    }
}
