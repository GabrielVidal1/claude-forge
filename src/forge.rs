//! Convert canonical conversations into Forge `context` rows and write them.
//!
//! The serde structs below mirror Forge's own
//! `forge_repo/src/conversation/conversation_record.rs` exactly (verified
//! against Forge v2.13.14 and against live `~/.forge/.forge.db` rows): the
//! `{"message":{"text"|"tool":{…}}}` wrapper, snake_case message tags, PascalCase
//! roles, camelCase tool values, and `{"actual":n}` token counts. We only need
//! the *serialize* direction to write rows; `Deserialize` is derived too so a
//! round-trip test can prove Forge will accept what we emit.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::canonical::{CanonicalConversation, CanonicalMessage, Role, ToolValue};
use crate::workspace_id::workspace_id;

// ---------------------------------------------------------------------------
// Record types mirroring forge_repo::conversation::conversation_record
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct ContextRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<ContextMessageRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ContextMessageRecord {
    message: ContextMessageValueRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<UsageRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContextMessageValueRecord {
    Text(TextMessageRecord),
    Tool(ToolResultRecord),
}

#[derive(Debug, Serialize, Deserialize)]
enum RoleRecord {
    System,
    User,
    Assistant,
}

#[derive(Debug, Serialize, Deserialize)]
struct TextMessageRecord {
    role: RoleRecord,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallFullRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_details: Option<Vec<ReasoningFullRecord>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolCallFullRecord {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    arguments: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReasoningFullRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_of: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolResultRecord {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    output: ToolOutputRecord,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolOutputRecord {
    is_error: bool,
    values: Vec<ToolValueRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ToolValueRecord {
    Text(String),
    Empty,
}

#[derive(Debug, Serialize, Deserialize)]
struct UsageRecord {
    prompt_tokens: TokenCountRecord,
    completion_tokens: TokenCountRecord,
    total_tokens: TokenCountRecord,
    cached_tokens: TokenCountRecord,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TokenCountRecord {
    Actual(usize),
    Approx(usize),
}

// ---------------------------------------------------------------------------
// Canonical -> ContextRecord
// ---------------------------------------------------------------------------

fn build_context(conv: &CanonicalConversation) -> ContextRecord {
    let messages = conv
        .messages
        .iter()
        .map(|m| match m {
            CanonicalMessage::Text {
                role,
                content,
                model,
                tool_calls,
                reasoning,
                usage,
                ..
            } => {
                let tool_calls = if tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        tool_calls
                            .iter()
                            .map(|c| ToolCallFullRecord {
                                name: c.name.clone(),
                                call_id: c.call_id.clone(),
                                arguments: c.arguments.clone(),
                            })
                            .collect(),
                    )
                };
                let reasoning_details = if reasoning.is_empty() {
                    None
                } else {
                    Some(
                        reasoning
                            .iter()
                            .map(|r| ReasoningFullRecord {
                                text: r.text.clone(),
                                signature: r.signature.clone(),
                                id: r.id.clone(),
                                type_of: Some("reasoning.text".to_string()),
                            })
                            .collect(),
                    )
                };
                ContextMessageRecord {
                    message: ContextMessageValueRecord::Text(TextMessageRecord {
                        role: role_record(*role),
                        content: content.clone(),
                        tool_calls,
                        model: model.clone(),
                        reasoning_details,
                    }),
                    usage: usage.map(|u| UsageRecord {
                        prompt_tokens: TokenCountRecord::Actual(u.prompt_tokens as usize),
                        completion_tokens: TokenCountRecord::Actual(u.completion_tokens as usize),
                        total_tokens: TokenCountRecord::Actual(u.total_tokens as usize),
                        cached_tokens: TokenCountRecord::Actual(u.cached_tokens as usize),
                    }),
                }
            }
            CanonicalMessage::ToolResult {
                name,
                call_id,
                is_error,
                values,
                ..
            } => ContextMessageRecord {
                message: ContextMessageValueRecord::Tool(ToolResultRecord {
                    name: name.clone(),
                    call_id: call_id.clone(),
                    output: ToolOutputRecord {
                        is_error: *is_error,
                        values: values
                            .iter()
                            .map(|v| match v {
                                ToolValue::Text(t) => ToolValueRecord::Text(t.clone()),
                                ToolValue::Empty => ToolValueRecord::Empty,
                            })
                            .collect(),
                    },
                }),
                usage: None,
            },
        })
        .collect();

    ContextRecord {
        conversation_id: Some(conv.id.clone()),
        messages,
    }
}

fn role_record(role: Role) -> RoleRecord {
    match role {
        Role::System => RoleRecord::System,
        Role::User => RoleRecord::User,
        Role::Assistant => RoleRecord::Assistant,
    }
}

/// Serialize a canonical conversation to a Forge `context` JSON string.
pub fn context_json(conv: &CanonicalConversation) -> Result<String> {
    serde_json::to_string(&build_context(conv)).context("serializing forge context")
}

/// Minimal Forge `metrics` blob: just the start time, no file changes.
fn metrics_json(created_at: DateTime<Utc>) -> String {
    json!({
        "started_at": created_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        "files_changed": {},
    })
    .to_string()
}

/// Forge stores diesel-naive UTC timestamps: `YYYY-MM-DD HH:MM:SS.ffffff`.
fn forge_ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string()
}

// ---------------------------------------------------------------------------
// Forge DB access
// ---------------------------------------------------------------------------

/// Open the Forge SQLite DB for reading + writing, with a busy timeout so we
/// never corrupt a DB that Forge might briefly touch.
pub fn open(forge_db: &Path) -> Result<Connection> {
    let conn = Connection::open(forge_db)
        .with_context(|| format!("opening forge db at {}", forge_db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    Ok(conn)
}

/// The set of `conversation_id`s already present in the Forge DB.
pub fn existing_ids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT conversation_id FROM conversations")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Insert a canonical conversation into Forge. Idempotent via `INSERT OR
/// IGNORE` on the `conversation_id` primary key.
pub fn insert(conn: &Connection, conv: &CanonicalConversation) -> Result<()> {
    let context = context_json(conv)?;
    let wid = workspace_id(conv.cwd.as_deref().unwrap_or(""));
    conn.execute(
        "INSERT OR IGNORE INTO conversations \
         (conversation_id, title, workspace_id, context, created_at, updated_at, metrics) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            conv.id,
            conv.title,
            wid,
            context,
            forge_ts(conv.created_at),
            forge_ts(conv.updated_at),
            metrics_json(conv.created_at),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{CanonicalMessage, CanonicalReasoning, CanonicalToolCall, Usage};
    use serde_json::Value;

    fn sample() -> CanonicalConversation {
        let ts = DateTime::parse_from_rfc3339("2026-06-01T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        CanonicalConversation {
            id: "abc".into(),
            title: Some("hi".into()),
            cwd: Some("/home/me/proj".into()),
            created_at: ts,
            updated_at: ts,
            messages: vec![
                CanonicalMessage::Text {
                    role: Role::User,
                    content: "hello".into(),
                    model: None,
                    tool_calls: vec![],
                    reasoning: vec![],
                    usage: None,
                    ts,
                },
                CanonicalMessage::Text {
                    role: Role::Assistant,
                    content: "running".into(),
                    model: Some("claude-x".into()),
                    tool_calls: vec![CanonicalToolCall {
                        name: "Bash".into(),
                        call_id: Some("tu_1".into()),
                        arguments: json!({"command": "ls"}),
                    }],
                    reasoning: vec![CanonicalReasoning {
                        text: Some("think".into()),
                        signature: Some("sig".into()),
                        id: None,
                    }],
                    usage: Some(Usage {
                        prompt_tokens: 17,
                        completion_tokens: 3,
                        total_tokens: 20,
                        cached_tokens: 5,
                    }),
                    ts,
                },
                CanonicalMessage::ToolResult {
                    name: "Bash".into(),
                    call_id: Some("tu_1".into()),
                    is_error: false,
                    values: vec![ToolValue::Text("file.txt".into())],
                    ts,
                },
            ],
        }
    }

    #[test]
    fn emits_the_exact_forge_shapes() {
        let v: Value = serde_json::from_str(&context_json(&sample()).unwrap()).unwrap();
        let msgs = v["messages"].as_array().unwrap();

        // User text turn.
        assert_eq!(msgs[0]["message"]["text"]["role"], "User");
        assert_eq!(msgs[0]["message"]["text"]["content"], "hello");

        // Assistant turn: tool_calls, model, reasoning_details, usage.
        let asst = &msgs[1];
        assert_eq!(asst["message"]["text"]["role"], "Assistant");
        assert_eq!(asst["message"]["text"]["model"], "claude-x");
        let call = &asst["message"]["text"]["tool_calls"][0];
        assert_eq!(call["name"], "Bash");
        assert_eq!(call["call_id"], "tu_1");
        assert_eq!(call["arguments"]["command"], "ls");
        assert_eq!(
            asst["message"]["text"]["reasoning_details"][0]["text"],
            "think"
        );
        assert_eq!(asst["usage"]["prompt_tokens"]["actual"], 17);
        assert_eq!(asst["usage"]["cached_tokens"]["actual"], 5);

        // Tool result turn.
        let tool = &msgs[2]["message"]["tool"];
        assert_eq!(tool["name"], "Bash");
        assert_eq!(tool["call_id"], "tu_1");
        assert_eq!(tool["output"]["is_error"], false);
        assert_eq!(tool["output"]["values"][0]["text"], "file.txt");
    }

    #[test]
    fn round_trips_through_the_record_types() {
        // What we emit must deserialize back through the same record structs —
        // a proxy for "Forge's lenient deserializer will accept it".
        let json = context_json(&sample()).unwrap();
        let record: ContextRecord = serde_json::from_str(&json).expect("re-deserialize");
        assert_eq!(record.conversation_id.as_deref(), Some("abc"));
        assert_eq!(record.messages.len(), 3);
    }

    #[test]
    fn forge_timestamp_format() {
        let ts = DateTime::parse_from_rfc3339("2026-06-01T10:00:00.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(forge_ts(ts), "2026-06-01 10:00:00.123456");
    }
}
