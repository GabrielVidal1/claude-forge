//! Read Claude Code's native JSONL session files into the canonical model.
//!
//! Claude Code stores one session per file at
//! `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, one JSON object per
//! line. We read the *native* files directly (not the lossy claude-vault
//! `vault.db`) so we can preserve tool calls, tool results, reasoning and usage.
//!
//! Parsing rules borrowed from claude-vault's `code_parser.py`:
//! - Skip `history.jsonl` and `agent-*.jsonl` (subagent transcripts — already
//!   embedded in the parent session as the Task tool call/result; treating them
//!   as separate conversations causes `sessionId` collisions).
//! - Skip entries of type `file-history-snapshot`, entries with `isMeta == true`,
//!   and string contents containing `<command-name>` / `<local-command-stdout>`.
//! - Keep `user`/`assistant` entries in file order (already causally ordered).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::canonical::{
    CanonicalConversation, CanonicalMessage, CanonicalReasoning, CanonicalToolCall, Role,
    ToolValue, Usage,
};

/// One parsed Claude session plus the provenance we keep in the sync DB.
pub struct ClaudeSession {
    pub canonical: CanonicalConversation,
    /// Original native bytes of the session file, kept verbatim for lossless
    /// re-export and upstream-edit detection.
    pub raw_blob: String,
    pub source_path: String,
}

/// Filters applied while reading sessions.
#[derive(Default)]
pub struct Filters {
    /// Only keep sessions whose `updated_at` is at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Only keep sessions whose `cwd` contains this substring.
    pub project: Option<String>,
}

/// Read every Claude session under `<claude_dir>/projects`, applying `filters`.
pub fn read_sessions(claude_dir: &Path, filters: &Filters) -> Result<Vec<ClaudeSession>> {
    let projects = claude_dir.join("projects");
    if !projects.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_jsonl(&projects, &mut files)?;
    files.sort();

    let mut out = Vec::new();
    for path in files {
        match parse_session_file(&path) {
            Ok(Some(session)) => {
                if filters.passes(&session) {
                    out.push(session);
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[claude-forge] skipping {}: {e:#}", path.display()),
        }
    }
    Ok(out)
}

impl Filters {
    fn passes(&self, s: &ClaudeSession) -> bool {
        if let Some(since) = self.since {
            if s.canonical.updated_at < since {
                return false;
            }
        }
        if let Some(project) = &self.project {
            match &s.canonical.cwd {
                Some(cwd) if cwd.contains(project) => {}
                _ => return false,
            }
        }
        true
    }
}

/// Recursively gather candidate `*.jsonl` files, skipping internal ones.
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && !is_internal_jsonl(&path)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// `history.jsonl` (a prompt index) and `agent-*.jsonl` (subagent transcripts)
/// are not standalone conversations.
fn is_internal_jsonl(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name == "history.jsonl" || name.starts_with("agent-"),
        None => true,
    }
}

/// Parse a single session file into a canonical conversation, or `None` if it
/// has no usable messages.
fn parse_session_file(path: &Path) -> Result<Option<ClaudeSession>> {
    let raw_blob =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut messages: Vec<CanonicalMessage> = Vec::new();
    let mut created_at: Option<DateTime<Utc>> = None;
    let mut updated_at: Option<DateTime<Utc>> = None;
    // tool_use_id -> tool name, so tool_result blocks can be labelled.
    let mut tool_names: HashMap<String, String> = HashMap::new();

    for line in raw_blob.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate the odd malformed line
        };

        if should_skip_entry(&entry) {
            continue;
        }

        if session_id.is_none() {
            if let Some(s) = entry.get("sessionId").and_then(Value::as_str) {
                session_id = Some(s.to_string());
            }
        }

        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if entry_type != "user" && entry_type != "assistant" {
            continue;
        }

        let ts = parse_timestamp(entry.get("timestamp"));

        let before = messages.len();
        match entry_type {
            "assistant" => parse_assistant(&entry, ts, &mut tool_names, &mut messages),
            "user" => parse_user(&entry, ts, &tool_names, &mut messages),
            _ => unreachable!(),
        }

        // Only advance timestamps / cwd if the entry actually produced content.
        if messages.len() > before {
            if cwd.is_none() {
                if let Some(c) = entry.get("cwd").and_then(Value::as_str) {
                    if !c.is_empty() {
                        cwd = Some(c.to_string());
                    }
                }
            }
            created_at = Some(created_at.map_or(ts, |c| c.min(ts)));
            updated_at = Some(updated_at.map_or(ts, |u| u.max(ts)));
        }
    }

    if messages.is_empty() {
        return Ok(None);
    }

    let id = session_id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });
    let created_at = created_at.unwrap_or_else(Utc::now);
    let updated_at = updated_at.unwrap_or(created_at);
    let title = title_from(&messages);

    Ok(Some(ClaudeSession {
        canonical: CanonicalConversation {
            id,
            title,
            cwd,
            created_at,
            updated_at,
            messages,
        },
        raw_blob,
        source_path: path.to_string_lossy().into_owned(),
    }))
}

/// Apply claude-vault's entry-level skip rules.
fn should_skip_entry(entry: &Value) -> bool {
    if entry.get("type").and_then(Value::as_str) == Some("file-history-snapshot") {
        return true;
    }
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if let Some(content) = entry
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
    {
        if content.contains("<command-name>") || content.contains("<local-command-stdout>") {
            return true;
        }
    }
    false
}

/// Build a single assistant `Text` message (with tool calls, reasoning, usage).
fn parse_assistant(
    entry: &Value,
    ts: DateTime<Utc>,
    tool_names: &mut HashMap<String, String>,
    out: &mut Vec<CanonicalMessage>,
) {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return,
    };

    let model = msg.get("model").and_then(Value::as_str).map(str::to_string);
    let usage = msg.get("usage").and_then(parse_usage);

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<CanonicalToolCall> = Vec::new();
    let mut reasoning: Vec<CanonicalReasoning> = Vec::new();

    match msg.get("content") {
        Some(Value::String(s)) => {
            if !s.is_empty() {
                text_parts.push(s.clone());
            }
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            if !t.is_empty() {
                                text_parts.push(t.to_string());
                            }
                        }
                    }
                    Some("thinking") => {
                        reasoning.push(CanonicalReasoning {
                            text: block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            signature: block
                                .get("signature")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            id: None,
                        });
                    }
                    Some("tool_use") => {
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let call_id = block.get("id").and_then(Value::as_str).map(str::to_string);
                        if let Some(id) = &call_id {
                            tool_names.insert(id.clone(), name.clone());
                        }
                        tool_calls.push(CanonicalToolCall {
                            name,
                            call_id,
                            arguments: block.get("input").cloned().unwrap_or(Value::Null),
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    // Emit the assistant turn if it carries anything at all (text-only,
    // thinking-only and tool-only turns are all valid).
    if text_parts.is_empty() && tool_calls.is_empty() && reasoning.is_empty() {
        return;
    }
    out.push(CanonicalMessage::Text {
        role: Role::Assistant,
        content: text_parts.join("\n\n"),
        model,
        tool_calls,
        reasoning,
        usage,
        ts,
    });
}

/// Emit a user `Text` message and/or one `ToolResult` per `tool_result` block.
fn parse_user(
    entry: &Value,
    ts: DateTime<Utc>,
    tool_names: &HashMap<String, String>,
    out: &mut Vec<CanonicalMessage>,
) {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return,
    };

    match msg.get("content") {
        Some(Value::String(s)) => {
            if !s.is_empty() {
                out.push(CanonicalMessage::Text {
                    role: Role::User,
                    content: s.clone(),
                    model: None,
                    tool_calls: Vec::new(),
                    reasoning: Vec::new(),
                    usage: None,
                    ts,
                });
            }
        }
        Some(Value::Array(blocks)) => {
            let mut text_parts: Vec<String> = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => {
                        let call_id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let name = call_id
                            .as_ref()
                            .and_then(|id| tool_names.get(id))
                            .cloned()
                            .unwrap_or_else(|| "tool".to_string());
                        let is_error = block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let text = stringify_tool_result(block.get("content"));
                        let values = if text.is_empty() {
                            vec![ToolValue::Empty]
                        } else {
                            vec![ToolValue::Text(text)]
                        };
                        out.push(CanonicalMessage::ToolResult {
                            name,
                            call_id,
                            is_error,
                            values,
                            ts,
                        });
                    }
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            if !t.is_empty() {
                                text_parts.push(t.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !text_parts.is_empty() {
                out.push(CanonicalMessage::Text {
                    role: Role::User,
                    content: text_parts.join("\n\n"),
                    model: None,
                    tool_calls: Vec::new(),
                    reasoning: Vec::new(),
                    usage: None,
                    ts,
                });
            }
        }
        _ => {}
    }
}

/// Flatten a `tool_result` `content` (string, array of text blocks, or other
/// JSON) into a single string.
fn stringify_tool_result(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => {
            let parts: Vec<String> = blocks
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(Value::as_str) == Some("text") {
                        b.get("text").and_then(Value::as_str).map(str::to_string)
                    } else {
                        b.as_str().map(str::to_string)
                    }
                })
                .collect();
            if parts.is_empty() {
                Value::Array(blocks.clone()).to_string()
            } else {
                parts.join("\n")
            }
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Map Claude's `usage` block into the canonical (all-"actual") form.
///
/// Claude reports `input_tokens` *excluding* cached/created tokens, so the true
/// prompt size is the sum of input + cache-creation + cache-read.
fn parse_usage(usage: &Value) -> Option<Usage> {
    let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input = n("input_tokens");
    let cache_create = n("cache_creation_input_tokens");
    let cache_read = n("cache_read_input_tokens");
    let output = n("output_tokens");

    // Ignore entries with no token data at all.
    if input == 0 && cache_create == 0 && cache_read == 0 && output == 0 {
        return None;
    }
    let prompt_tokens = input + cache_create + cache_read;
    Some(Usage {
        prompt_tokens,
        completion_tokens: output,
        total_tokens: prompt_tokens + output,
        cached_tokens: cache_read,
    })
}

/// Parse a Claude timestamp — ISO-8601 string or epoch-milliseconds number.
fn parse_timestamp(ts: Option<&Value>) -> DateTime<Utc> {
    match ts {
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        Some(Value::Number(n)) => n
            .as_i64()
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now),
        _ => Utc::now(),
    }
}

/// First user line, whitespace-collapsed and truncated to 70 chars + ellipsis.
fn title_from(messages: &[CanonicalMessage]) -> Option<String> {
    messages.iter().find_map(|m| match m {
        CanonicalMessage::Text {
            role: Role::User,
            content,
            ..
        } => {
            let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
            if collapsed.is_empty() {
                None
            } else if collapsed.chars().count() > 70 {
                let head: String = collapsed.chars().take(70).collect();
                Some(format!("{head}…"))
            } else {
                Some(collapsed)
            }
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn parse_str(jsonl: &str) -> Option<ClaudeSession> {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        f.write_all(jsonl.as_bytes()).unwrap();
        parse_session_file(f.path()).unwrap()
    }

    #[test]
    fn parses_text_thinking_tooluse_and_result() {
        let jsonl = r#"
{"type":"user","sessionId":"s1","cwd":"/home/me/proj","timestamp":"2026-06-01T10:00:00.000Z","message":{"role":"user","content":"do the thing"}}
{"type":"assistant","sessionId":"s1","timestamp":"2026-06-01T10:00:01.000Z","message":{"model":"claude-x","role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"on it"},{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":2,"output_tokens":3}}}
{"type":"user","sessionId":"s1","timestamp":"2026-06-01T10:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":false,"content":"file.txt"}]}}
"#;
        let s = parse_str(jsonl).expect("session");
        assert_eq!(s.canonical.id, "s1");
        assert_eq!(s.canonical.cwd.as_deref(), Some("/home/me/proj"));
        assert_eq!(s.canonical.title.as_deref(), Some("do the thing"));
        assert_eq!(s.canonical.messages.len(), 3);

        match &s.canonical.messages[1] {
            CanonicalMessage::Text {
                role,
                content,
                model,
                tool_calls,
                reasoning,
                usage,
                ..
            } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(content, "on it");
                assert_eq!(model.as_deref(), Some("claude-x"));
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "Bash");
                assert_eq!(tool_calls[0].call_id.as_deref(), Some("tu_1"));
                assert_eq!(reasoning.len(), 1);
                assert_eq!(reasoning[0].text.as_deref(), Some("hmm"));
                let u = usage.expect("usage");
                assert_eq!(u.prompt_tokens, 17); // 10 + 2 + 5
                assert_eq!(u.cached_tokens, 5);
                assert_eq!(u.completion_tokens, 3);
                assert_eq!(u.total_tokens, 20);
            }
            other => panic!("expected assistant text, got {other:?}"),
        }

        match &s.canonical.messages[2] {
            CanonicalMessage::ToolResult {
                name,
                call_id,
                is_error,
                values,
                ..
            } => {
                assert_eq!(name, "Bash"); // resolved from tool_use map
                assert_eq!(call_id.as_deref(), Some("tu_1"));
                assert!(!is_error);
                assert!(matches!(&values[0], ToolValue::Text(t) if t == "file.txt"));
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[test]
    fn skips_meta_and_command_and_snapshot_entries() {
        let jsonl = r#"
{"type":"file-history-snapshot","sessionId":"s2"}
{"type":"user","isMeta":true,"sessionId":"s2","timestamp":"2026-06-01T10:00:00Z","message":{"role":"user","content":"meta"}}
{"type":"user","sessionId":"s2","timestamp":"2026-06-01T10:00:01Z","message":{"role":"user","content":"<command-name>/foo</command-name>"}}
{"type":"user","sessionId":"s2","timestamp":"2026-06-01T10:00:02Z","message":{"role":"user","content":"real prompt"}}
"#;
        let s = parse_str(jsonl).expect("session");
        assert_eq!(s.canonical.messages.len(), 1);
        assert_eq!(s.canonical.title.as_deref(), Some("real prompt"));
    }

    #[test]
    fn empty_session_is_none() {
        assert!(parse_str("{\"type\":\"system\"}\n").is_none());
    }
}
