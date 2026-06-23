//! Copy claude-vault sessions into Forge's `conversations` table.
//!
//! claude-vault archives Claude Code sessions in a SQLite DB (one row per
//! message: `session_id, role, content, timestamp`). Forge stores each whole
//! conversation as a single row in its own SQLite DB, serialising the
//! transcript as a JSON `context` blob.
//!
//! The import is idempotent: the vault `session_id` (a UUID) is reused verbatim
//! as the Forge `conversation_id`, so a session is inserted at most once and the
//! tool is safe to run on every Forge launch.
//!
//! Known lossiness: vault only keeps user/assistant *text* (tool calls are
//! flattened into the assistant content string, e.g. `[tool_use: Bash] {…}`);
//! there are no separate tool-result or usage records. Imported conversations
//! are therefore readable plain-text transcripts, not byte-perfect Forge
//! contexts.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

use crate::workspace_id::{resolve_cwd, workspace_id};

#[derive(Debug, Default)]
pub struct Report {
    pub inserted: usize,
    pub skipped: usize,
    pub empty: usize,
}

struct Session {
    session_id: String,
    project: String,
    started_at: Option<String>,
}

struct Message {
    role: String,
    content: String,
    timestamp: Option<String>,
}

/// Run the import. When `dry_run` is true nothing is written to the Forge DB.
pub fn import(vault_db: &Path, forge_db: &Path, dry_run: bool) -> Result<Report> {
    let vault = Connection::open_with_flags(
        vault_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening vault db at {}", vault_db.display()))?;

    let forge = Connection::open(forge_db)
        .with_context(|| format!("opening forge db at {}", forge_db.display()))?;
    forge.busy_timeout(std::time::Duration::from_secs(30))?;

    let existing = existing_conversation_ids(&forge)?;
    let sessions = load_sessions(&vault)?;

    // Cache cwd -> workspace_id; filesystem resolution is the slow part.
    let mut wid_cache: HashMap<String, i64> = HashMap::new();
    let mut report = Report::default();

    let tx = forge.unchecked_transaction()?;
    for s in &sessions {
        if existing.contains(&s.session_id) {
            report.skipped += 1;
            continue;
        }
        let rows = load_messages(&vault, &s.session_id)?;
        if rows.is_empty() {
            report.empty += 1;
            continue;
        }

        let cwd = resolve_cwd(&s.project);
        let wid = *wid_cache
            .entry(cwd.clone())
            .or_insert_with(|| workspace_id(&cwd));
        let ctx = build_context(&s.session_id, &rows);
        let created = to_forge_ts(s.started_at.as_deref().or(rows[0].timestamp.as_deref()));
        let updated = to_forge_ts(rows.last().and_then(|m| m.timestamp.as_deref()));

        if !dry_run {
            tx.execute(
                "INSERT OR IGNORE INTO conversations \
                 (conversation_id, title, workspace_id, context, created_at, updated_at, metrics) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                rusqlite::params![s.session_id, title_for(&rows), wid, ctx, created, updated],
            )?;
        }
        report.inserted += 1;
    }
    if dry_run {
        // Nothing was written, but commit the empty tx cleanly.
        tx.rollback()?;
    } else {
        tx.commit()?;
    }

    Ok(report)
}

fn existing_conversation_ids(forge: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = forge.prepare("SELECT conversation_id FROM conversations")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn load_sessions(vault: &Connection) -> Result<Vec<Session>> {
    let mut stmt =
        vault.prepare("SELECT session_id, project, started_at FROM sessions ORDER BY started_at")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Session {
                session_id: r.get(0)?,
                project: r.get(1)?,
                started_at: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn load_messages(vault: &Connection, session_id: &str) -> Result<Vec<Message>> {
    let mut stmt = vault.prepare(
        "SELECT role, content, timestamp FROM messages WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok(Message {
                role: r.get(0)?,
                content: r.get(1)?,
                timestamp: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Serialise vault messages into a Forge `context` JSON blob.
fn build_context(session_id: &str, rows: &[Message]) -> String {
    let messages: Vec<_> = rows
        .iter()
        .map(|m| {
            let role = match m.role.to_lowercase().as_str() {
                "assistant" => "Assistant",
                "system" => "System",
                _ => "User",
            };
            json!({ "message": { "text": { "role": role, "content": m.content } } })
        })
        .collect();
    json!({ "conversation_id": session_id, "messages": messages }).to_string()
}

/// First user line, collapsed and truncated, used as the conversation title.
fn title_for(rows: &[Message]) -> Option<String> {
    rows.iter()
        .find(|m| m.role.eq_ignore_ascii_case("user"))
        .map(|m| {
            let t = m.content.split_whitespace().collect::<Vec<_>>().join(" ");
            if t.chars().count() > 70 {
                let head: String = t.chars().take(70).collect();
                format!("{head}…")
            } else {
                t
            }
        })
}

/// vault ISO8601 (`…Z`) -> diesel naive `YYYY-MM-DD HH:MM:SS.ffffff` (UTC).
fn to_forge_ts(iso: Option<&str>) -> String {
    let dt: DateTime<Utc> = iso
        .and_then(|s| {
            DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
        .unwrap_or_else(Utc::now);
    // Drop the trailing 'Z'/offset and force 6-digit microseconds to match diesel.
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string()
}
