//! Reproduce Forge's `workspace_id` for a directory path.
//!
//! A Forge conversation only shows up in a workspace if its `workspace_id`
//! equals Forge's hash of that workspace's cwd. Forge derives it by hashing the
//! `PathBuf` with the std `DefaultHasher` (SipHash-1-3, keys 0,0) and storing
//! `hash as i64` in the `workspace_id` BigInt column.
//!
//! Because this is a pure-std computation we run it inline — the original
//! Python importer had to shell out to a separate Rust helper binary to get
//! this exact value; doing the whole tool in Rust collapses both jobs into one.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Compute the Forge `workspace_id` (as stored, an `i64`) for a directory path.
pub fn workspace_id(cwd: &str) -> i64 {
    let mut hasher = DefaultHasher::new();
    PathBuf::from(cwd).hash(&mut hasher);
    hasher.finish() as i64
}

/// Reconstruct a real cwd from claude-vault's dash-encoded `project` name.
///
/// Claude Code encodes a path like `/home/gabrielvidal/homelab` as
/// `-home-gabrielvidal-homelab`. Because a literal `-` in a path component and
/// the separator `/` both become `-`, decoding is ambiguous, so we resolve
/// greedily against the filesystem: at each level pick the longest run of
/// segments that names an existing directory. We fall back to a literal
/// one-segment step when nothing matches (e.g. the directory has since been
/// deleted) — the resulting path string still hashes deterministically, it just
/// may not match a live workspace.
///
/// Kept available (not used by the Claude→Forge path, which has the real `cwd`)
/// as a fallback for future sources that only carry the dash-encoded name.
#[allow(dead_code)]
pub fn resolve_cwd(project: &str) -> String {
    let segs: Vec<&str> = project.split('-').filter(|s| !s.is_empty()).collect();
    let mut cur = PathBuf::from("/");
    let mut i = 0;
    while i < segs.len() {
        let mut matched = false;
        let mut j = segs.len();
        while j > i {
            let cand = segs[i..j].join("-");
            if cur.join(&cand).is_dir() {
                cur.push(&cand);
                i = j;
                matched = true;
                break;
            }
            j -= 1;
        }
        if !matched {
            cur.push(segs[i]);
            i += 1;
        }
    }
    path_to_string(&cur)
}

#[allow(dead_code)]
fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Validated against real Forge rows on the author's machine.
    #[test]
    fn known_workspace_ids() {
        assert_eq!(workspace_id("/home/gabrielvidal"), -8599109238221935417);
        assert_eq!(
            workspace_id("/home/gabrielvidal/homelab"),
            8968329562854484240
        );
        assert_eq!(
            workspace_id("/home/gabrielvidal/homelab/projects/zipgo"),
            -3877205949088219147
        );
    }
}
