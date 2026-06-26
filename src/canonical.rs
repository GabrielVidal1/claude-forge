//! The agent-neutral canonical conversation model.
//!
//! Both Claude Code and Forge map to/from this intermediate representation. It
//! is a deliberate "superset-lite": it carries everything Forge needs and
//! everything Claude readily provides (tool calls, tool results, reasoning,
//! per-message usage). Anything an agent cannot represent is preserved only in
//! the raw blob stored alongside it in the sync DB (see [`crate::state`]), never
//! lost.
//!
//! The canonical form is what we serialize into `conversations.canonical_json`
//! and hash on to detect semantic changes independent of raw formatting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalConversation {
    /// Stable canonical id. For Claude-sourced conversations this is the
    /// `sessionId`, reused verbatim as Forge's `conversation_id`.
    pub id: String,
    pub title: Option<String>,
    /// Drives the Forge `workspace_id`; taken from the Claude entry's `cwd`.
    pub cwd: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<CanonicalMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalMessage {
    /// A user / assistant / system text turn (with any assistant tool calls,
    /// reasoning and usage attached).
    Text {
        role: Role,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<CanonicalToolCall>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasoning: Vec<CanonicalReasoning>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        ts: DateTime<Utc>,
    },
    /// The result of a tool call.
    ToolResult {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        is_error: bool,
        values: Vec<ToolValue>,
        ts: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalToolCall {
    pub name: String,
    pub call_id: Option<String>,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalReasoning {
    pub text: Option<String>,
    pub signature: Option<String>,
    pub id: Option<String>,
}

/// Per-message token usage, normalized across agents. All counts are "actual".
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolValue {
    Text(String),
    Empty,
}
