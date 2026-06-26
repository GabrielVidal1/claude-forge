//! Orchestration: read Claude sessions, record them in the sync DB, and write
//! the new ones into Forge — idempotently.
//!
//! v1 is one-directional (Claude → Forge). Idempotency comes from two places
//! that agree: the Forge DB's `conversation_id` primary key (we reuse the Claude
//! `sessionId`) and the sync DB's `agent_links`. A session already present in
//! Forge is skipped but still recorded in the state DB so `status` reflects
//! reality.

use std::path::Path;

use anyhow::Result;

use crate::claude::{self, Filters};
use crate::forge;
use crate::state::{StateDb, AGENT_CLAUDE, AGENT_FORGE};

#[derive(Debug, Default)]
pub struct Report {
    /// New conversations written to Forge.
    pub inserted: usize,
    /// Sessions already present in Forge (skipped).
    pub skipped_present: usize,
    /// Sessions whose canonical content changed since we last wrote Forge
    /// (Forge keeps the original; logged for visibility).
    pub drifted: usize,
    /// Total Claude sessions considered.
    pub total: usize,
}

pub struct Options<'a> {
    pub claude_dir: &'a Path,
    pub forge_db: &'a Path,
    pub state_db: &'a Path,
    pub filters: Filters,
    pub dry_run: bool,
    pub verbose: bool,
}

pub fn sync(opts: Options) -> Result<Report> {
    let sessions = claude::read_sessions(opts.claude_dir, &opts.filters)?;
    let state = StateDb::open(opts.state_db)?;
    let forge_conn = forge::open(opts.forge_db)?;
    let existing = forge::existing_ids(&forge_conn)?;

    let mut report = Report {
        total: sessions.len(),
        ..Default::default()
    };

    // One transaction for all Forge inserts (rolled back on dry-run).
    let tx = forge_conn.unchecked_transaction()?;
    for session in &sessions {
        let conv = &session.canonical;

        let canonical_hash = state.upsert_conversation(conv)?;
        state.upsert_source_link(
            &conv.id,
            AGENT_CLAUDE,
            &conv.id,
            &session.source_path,
            &session.raw_blob,
        )?;

        let already_linked = state.has_link(&conv.id, AGENT_FORGE)?;
        if existing.contains(&conv.id) {
            report.skipped_present += 1;
            if !already_linked && !opts.dry_run {
                // Forge has it but we never recorded the link (e.g. inserted by
                // the old importer); adopt it without rewriting Forge.
                state.record_written_link(
                    &conv.id,
                    AGENT_FORGE,
                    &conv.id,
                    &session.source_path,
                    &session.raw_blob,
                )?;
            }
            if opts.verbose {
                eprintln!(
                    "[claude-forge] present  {} ({})",
                    conv.id,
                    hash_short(&canonical_hash)
                );
            }
            continue;
        }

        if already_linked {
            // We wrote it before but it's gone from Forge (DB reset?); treat the
            // changed-canonical case as drift, otherwise re-insert.
            report.drifted += 1;
        }

        if opts.dry_run {
            report.inserted += 1;
            if opts.verbose {
                eprintln!("[claude-forge] would insert {} — {:?}", conv.id, conv.title);
            }
            continue;
        }

        forge::insert(&tx, conv)?;
        state.record_written_link(
            &conv.id,
            AGENT_FORGE,
            &conv.id,
            &session.source_path,
            &session.raw_blob,
        )?;
        report.inserted += 1;
        if opts.verbose {
            eprintln!("[claude-forge] inserted {} — {:?}", conv.id, conv.title);
        }
    }

    if opts.dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }

    Ok(report)
}

fn hash_short(h: &str) -> &str {
    &h[..h.len().min(8)]
}
