//! claude-to-forge — import Claude Code conversations archived by claude-vault
//! into the Forge (forgecode) database, so past Claude Code chats appear in
//! Forge's history.
//!
//! A single self-contained binary that does both jobs the original two-part
//! tool needed: it reproduces Forge's `workspace_id` natively (no helper
//! binary) and performs the idempotent import.

mod import;
mod workspace_id;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Default location of the claude-vault SQLite archive.
fn default_vault_db() -> PathBuf {
    home().join(".local/share/claude-vault/vault.db")
}

/// Default location of the Forge SQLite database.
fn default_forge_db() -> PathBuf {
    home().join(".forge/.forge.db")
}

fn home() -> PathBuf {
    dirs::home_dir().expect("could not determine home directory")
}

#[derive(Parser)]
#[command(
    name = "claude-to-forge",
    version,
    about = "Import claude-vault conversations into the Forge database",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    import: ImportArgs,
}

#[derive(clap::Args)]
struct ImportArgs {
    /// Path to the claude-vault SQLite archive.
    #[arg(long, env = "VAULT_DB")]
    vault_db: Option<PathBuf>,

    /// Path to the Forge SQLite database.
    #[arg(long, env = "FORGE_DB")]
    forge_db: Option<PathBuf>,

    /// Show what would be imported without writing to the Forge DB.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Import any new claude-vault sessions into the Forge DB (default).
    Import(ImportArgs),
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
        Some(Command::Import(args)) => run_import(args),
        None => run_import(cli.import),
    }
}

fn run_import(args: ImportArgs) -> Result<()> {
    let vault_db = args.vault_db.unwrap_or_else(default_vault_db);
    let forge_db = args.forge_db.unwrap_or_else(default_forge_db);

    if !vault_db.exists() {
        eprintln!(
            "[claude-to-forge] vault db not found at {} — nothing to import",
            vault_db.display()
        );
        return Ok(());
    }
    if !forge_db.exists() {
        eprintln!(
            "[claude-to-forge] forge db not found at {} — run forge once first",
            forge_db.display()
        );
        return Ok(());
    }

    let report = import::import(&vault_db, &forge_db, args.dry_run)?;
    let prefix = if args.dry_run {
        "[claude-to-forge] (dry-run) would insert"
    } else {
        "[claude-to-forge] inserted"
    };
    eprintln!(
        "{} {}, already-present {}, empty-skipped {}",
        prefix, report.inserted, report.skipped, report.empty
    );
    Ok(())
}
