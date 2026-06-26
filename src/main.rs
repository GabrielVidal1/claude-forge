//! claude-forge — move AI coding-agent conversations between Claude Code and
//! Forge (forgecode) with full metadata, backed by a local sync-state database.
//!
//! v1 wires the Claude → Forge direction: it reads Claude Code's native JSONL
//! session files directly and writes rich Forge `context` rows that preserve
//! tool calls, tool results, usage and reasoning — keeping a `sync.db` so
//! repeated runs are idempotent. The architecture is agent-neutral so more
//! agents and the reverse direction can be added without redesign.
//!
//! See `PLAN.md` for the full design and the data-format references.

mod canonical;
mod claude;
mod forge;
mod state;
mod sync;
mod workspace_id;

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand};

use claude::Filters;
use state::StateDb;

fn home() -> PathBuf {
    dirs::home_dir().expect("could not determine home directory")
}

fn default_claude_dir() -> PathBuf {
    home().join(".claude")
}

fn default_forge_db() -> PathBuf {
    home().join(".forge/.forge.db")
}

/// `$XDG_DATA_HOME/claude-forge/sync.db` (falls back to `~/.local/share`).
fn default_state_db() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home().join(".local/share"))
        .join("claude-forge/sync.db")
}

#[derive(Parser)]
#[command(
    name = "claude-forge",
    version,
    about = "Sync Claude Code conversations into Forge with full metadata",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    sync: SyncArgs,
}

#[derive(clap::Args)]
struct SyncArgs {
    /// Path to the Claude Code config directory (contains `projects/`).
    #[arg(long, env = "CLAUDE_DIR")]
    claude_dir: Option<PathBuf>,

    /// Path to the Forge SQLite database.
    #[arg(long, env = "FORGE_DB")]
    forge_db: Option<PathBuf>,

    /// Path to claude-forge's sync-state database.
    #[arg(long, env = "CLAUDE_FORGE_DB")]
    state_db: Option<PathBuf>,

    /// Only sync sessions updated on or after this date (YYYY-MM-DD).
    #[arg(long)]
    since: Option<String>,

    /// Only sync sessions whose working directory contains this substring.
    #[arg(long)]
    project: Option<String>,

    /// Report what would happen without writing to any database.
    #[arg(long)]
    dry_run: bool,

    /// Verbose per-session logging.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Sync new Claude Code sessions into Forge (default).
    Sync(SyncArgs),
    /// Show what is tracked: per-agent counts and the Forge DB row count.
    Status(SyncArgs),
    /// Print the Forge workspace_id for a directory path.
    WorkspaceId {
        /// Directory path (defaults to the current working directory).
        path: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::WorkspaceId { path }) => {
            let cwd = match path {
                Some(p) => p,
                None => std::env::current_dir()?.to_string_lossy().into_owned(),
            };
            println!("{}", workspace_id::workspace_id(&cwd));
            Ok(())
        }
        Some(Command::Status(args)) => run_status(args),
        Some(Command::Sync(args)) => run_sync(args),
        None => run_sync(cli.sync),
    }
}

fn resolve(args: &SyncArgs) -> (PathBuf, PathBuf, PathBuf) {
    (
        args.claude_dir.clone().unwrap_or_else(default_claude_dir),
        args.forge_db.clone().unwrap_or_else(default_forge_db),
        args.state_db.clone().unwrap_or_else(default_state_db),
    )
}

/// Parse `--since YYYY-MM-DD` into a UTC start-of-day instant.
fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid --since date {s:?} (expected YYYY-MM-DD)"))?;
    Ok(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap()))
}

fn run_sync(args: SyncArgs) -> Result<()> {
    let (claude_dir, forge_db, state_db) = resolve(&args);

    if !claude_dir.exists() {
        eprintln!(
            "[claude-forge] claude dir not found at {} — nothing to sync",
            claude_dir.display()
        );
        return Ok(());
    }
    if !forge_db.exists() {
        eprintln!(
            "[claude-forge] forge db not found at {} — run forge once first",
            forge_db.display()
        );
        return Ok(());
    }

    let filters = Filters {
        since: args.since.as_deref().map(parse_since).transpose()?,
        project: args.project.clone(),
    };

    let report = sync::sync(sync::Options {
        claude_dir: &claude_dir,
        forge_db: &forge_db,
        state_db: &state_db,
        filters,
        dry_run: args.dry_run,
        verbose: args.verbose,
    })?;

    let verb = if args.dry_run {
        "(dry-run) would insert"
    } else {
        "inserted"
    };
    eprintln!(
        "[claude-forge] {} {} of {} sessions; already-present {}, drift {}",
        verb, report.inserted, report.total, report.skipped_present, report.drifted
    );
    Ok(())
}

fn run_status(args: SyncArgs) -> Result<()> {
    let (_claude_dir, forge_db, state_db) = resolve(&args);

    let state = StateDb::open(&state_db)?;
    let counts = state.counts()?;

    let forge_rows: i64 = if forge_db.exists() {
        let conn = forge::open(&forge_db)?;
        conn.query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?
    } else {
        -1
    };

    println!("state db: {}", state_db.display());
    println!("  conversations tracked : {}", counts.conversations);
    println!("  claude links          : {}", counts.claude_links);
    println!("  forge links (written) : {}", counts.forge_links);
    println!("forge db: {}", forge_db.display());
    if forge_rows >= 0 {
        println!("  total conversations   : {forge_rows}");
    } else {
        println!("  (not found)");
    }
    Ok(())
}
