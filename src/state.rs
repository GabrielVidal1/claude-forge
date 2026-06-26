//! The sync-state database: canonical conversations + per-agent provenance.
//!
//! This is a SQLite DB owned by claude-forge (default
//! `$XDG_DATA_HOME/claude-forge/sync.db`). It stores, for every logical
//! conversation:
//! - the normalized [`CanonicalConversation`] and a hash of it, and
//! - one `agent_links` row per agent (`claude` / `forge`) holding the *original*
//!   native blob and its hash.
//!
//! The raw blob is the "agent harness" we keep so we can re-export losslessly,
//! detect upstream edits via the hash, and debug mapping bugs against ground
//! truth. The schema is designed for bidirectional sync; v1 only writes the
//! Claude→Forge direction.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::canonical::CanonicalConversation;

pub const AGENT_CLAUDE: &str = "claude";
pub const AGENT_FORGE: &str = "forge";

pub struct StateDb {
    conn: Connection,
}

/// sha256 hex digest of a string.
pub fn hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

impl StateDb {
    /// Open (creating + migrating if needed) the sync DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening state db at {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                 canonical_id   TEXT PRIMARY KEY,
                 title          TEXT,
                 cwd            TEXT,
                 created_at     TEXT NOT NULL,
                 updated_at     TEXT NOT NULL,
                 canonical_json TEXT NOT NULL,
                 canonical_hash TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS agent_links (
                 canonical_id   TEXT NOT NULL REFERENCES conversations(canonical_id),
                 agent          TEXT NOT NULL,
                 native_id      TEXT NOT NULL,
                 source_path    TEXT,
                 raw_blob       TEXT NOT NULL,
                 raw_hash       TEXT NOT NULL,
                 last_seen      TEXT NOT NULL,
                 last_written   TEXT,
                 PRIMARY KEY (agent, native_id)
             );
             CREATE INDEX IF NOT EXISTS idx_agent_links_canonical
                 ON agent_links(canonical_id);",
        )?;
        Ok(())
    }

    /// Upsert the canonical conversation. Returns its `canonical_hash`.
    pub fn upsert_conversation(&self, conv: &CanonicalConversation) -> Result<String> {
        let canonical_json = serde_json::to_string(conv)?;
        let canonical_hash = hash(&canonical_json);
        self.conn.execute(
            "INSERT INTO conversations
                 (canonical_id, title, cwd, created_at, updated_at, canonical_json, canonical_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(canonical_id) DO UPDATE SET
                 title=excluded.title, cwd=excluded.cwd,
                 created_at=excluded.created_at, updated_at=excluded.updated_at,
                 canonical_json=excluded.canonical_json,
                 canonical_hash=excluded.canonical_hash",
            rusqlite::params![
                conv.id,
                conv.title,
                conv.cwd,
                conv.created_at.to_rfc3339(),
                conv.updated_at.to_rfc3339(),
                canonical_json,
                canonical_hash,
            ],
        )?;
        Ok(canonical_hash)
    }

    /// Record (or refresh) the source-agent link, storing the raw native blob.
    pub fn upsert_source_link(
        &self,
        canonical_id: &str,
        agent: &str,
        native_id: &str,
        source_path: &str,
        raw_blob: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO agent_links
                 (canonical_id, agent, native_id, source_path, raw_blob, raw_hash, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(agent, native_id) DO UPDATE SET
                 canonical_id=excluded.canonical_id,
                 source_path=excluded.source_path,
                 raw_blob=excluded.raw_blob,
                 raw_hash=excluded.raw_hash,
                 last_seen=excluded.last_seen",
            rusqlite::params![
                canonical_id,
                agent,
                native_id,
                source_path,
                raw_blob,
                hash(raw_blob),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Record that we wrote this conversation to the target agent.
    pub fn record_written_link(
        &self,
        canonical_id: &str,
        agent: &str,
        native_id: &str,
        source_path: &str,
        raw_blob: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO agent_links
                 (canonical_id, agent, native_id, source_path, raw_blob, raw_hash, last_seen, last_written)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(agent, native_id) DO UPDATE SET
                 canonical_id=excluded.canonical_id,
                 source_path=excluded.source_path,
                 raw_blob=excluded.raw_blob,
                 raw_hash=excluded.raw_hash,
                 last_seen=excluded.last_seen,
                 last_written=excluded.last_written",
            rusqlite::params![
                canonical_id,
                agent,
                native_id,
                source_path,
                raw_blob,
                hash(raw_blob),
                now,
            ],
        )?;
        Ok(())
    }

    /// Whether a link to `agent` already exists for this canonical conversation.
    pub fn has_link(&self, canonical_id: &str, agent: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM agent_links WHERE canonical_id = ?1 AND agent = ?2 LIMIT 1",
                rusqlite::params![canonical_id, agent],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Per-agent link counts, for the `status` command.
    pub fn counts(&self) -> Result<Counts> {
        let conversations: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?;
        let claude_links: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM agent_links WHERE agent = ?1",
            [AGENT_CLAUDE],
            |r| r.get(0),
        )?;
        let forge_links: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM agent_links WHERE agent = ?1",
            [AGENT_FORGE],
            |r| r.get(0),
        )?;
        Ok(Counts {
            conversations,
            claude_links,
            forge_links,
        })
    }
}

#[derive(Debug)]
pub struct Counts {
    pub conversations: i64,
    pub claude_links: i64,
    pub forge_links: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn conv(id: &str) -> CanonicalConversation {
        CanonicalConversation {
            id: id.into(),
            title: Some("t".into()),
            cwd: Some("/x".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            messages: vec![],
        }
    }

    #[test]
    fn upsert_and_link_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(&dir.path().join("sync.db")).unwrap();

        db.upsert_conversation(&conv("c1")).unwrap();
        assert!(!db.has_link("c1", AGENT_FORGE).unwrap());

        db.upsert_source_link("c1", AGENT_CLAUDE, "c1", "/p.jsonl", "rawblob")
            .unwrap();
        db.record_written_link("c1", AGENT_FORGE, "c1", "/p.jsonl", "rawblob")
            .unwrap();
        assert!(db.has_link("c1", AGENT_FORGE).unwrap());

        // Idempotent: re-running does not duplicate rows.
        db.upsert_conversation(&conv("c1")).unwrap();
        db.record_written_link("c1", AGENT_FORGE, "c1", "/p.jsonl", "rawblob")
            .unwrap();

        let counts = db.counts().unwrap();
        assert_eq!(counts.conversations, 1);
        assert_eq!(counts.claude_links, 1);
        assert_eq!(counts.forge_links, 1);
    }
}
