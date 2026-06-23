//! forge-workspace-id — print the forgecode `workspace_id` for a directory path.
//!
//! Forge derives a conversation's `workspace_id` by hashing the current working
//! directory with the std `DefaultHasher` (SipHash-1-3, keys 0,0) via
//! `PathBuf::hash`, then storing `hash as i64`. Reproducing that hash by hand
//! (SipHash + the path-normalising `Path::hash`) is fragile, so we just call the
//! exact std implementation forge itself uses. std-only: compile with plain
//! `rustc -O workspace_id.rs -o forge-workspace-id`.
//!
//! Usage: forge-workspace-id <path>   # prints the i64 workspace_id on stdout

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: forge-workspace-id <path>");
            std::process::exit(2);
        }
    };
    let mut hasher = DefaultHasher::new();
    PathBuf::from(path).hash(&mut hasher);
    // Match forge: `WorkspaceHash(u64).id() as i64` stored in the BigInt column.
    println!("{}", hasher.finish() as i64);
}
