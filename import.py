#!/usr/bin/env python3
"""Import Claude Code conversations from claude-vault into the forgecode DB.

claude-vault (https://… local tool) archives Claude Code JSONL sessions into a
SQLite DB with FTS5. forgecode keeps its conversations in its own SQLite DB,
serialising each whole conversation as a JSON `context` blob in one row.

This script copies every vault session that forge doesn't already have into
forge's `conversations` table, so past Claude Code chats show up in forge's
history. It is idempotent (safe to run before every `forge` launch): the vault
`session_id` (a UUID) is reused verbatim as the forge `conversation_id`, so a
session is inserted at most once.

Mapping notes / known lossiness:
  * vault only stores user/assistant *text* (tool calls are flattened into the
    assistant content string, e.g. `[tool_use: Bash] {…}`); there are no
    separate tool-result or usage records. We therefore emit plain forge
    `text` messages and omit `usage`. The transcript is readable but not a
    byte-perfect forge context.
  * `workspace_id` must equal forge's `DefaultHasher`/`PathBuf::hash` of the
    workspace cwd, so a conversation lists under the right workspace. We shell
    out to the std-only `forge-workspace-id` binary (built from
    workspace_id.rs) — reimplementing SipHash-1-3 + Path::hash in Python is
    fragile. The cwd is reconstructed from vault's dash-encoded `project`
    name by greedily matching against the real filesystem.

Env overrides: VAULT_DB, FORGE_DB, FORGE_WORKSPACE_ID (path to the helper bin).
"""
from __future__ import annotations

import json
import os
import shutil
import sqlite3
import subprocess
import sys
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path

HOME = Path.home()
VAULT_DB = Path(os.environ.get("VAULT_DB", HOME / ".local/share/claude-vault/vault.db"))
FORGE_DB = Path(os.environ.get("FORGE_DB", HOME / ".forge/.forge.db"))
WID_BIN = os.environ.get("FORGE_WORKSPACE_ID") or shutil.which("forge-workspace-id") or str(
    Path(__file__).resolve().parent / "forge-workspace-id"
)


def log(msg: str) -> None:
    print(f"[forge-vault-import] {msg}", file=sys.stderr)


def resolve_cwd(project: str) -> str:
    """Reconstruct a real cwd from claude-vault's dash-encoded project name.

    Claude Code encodes a path like /home/gabrielvidal/homelab as
    `-home-gabrielvidal-homelab`. Because real `-` and the path `/` both become
    `-`, decoding is ambiguous, so we resolve greedily against the filesystem:
    at each level pick the longest run of segments that names an existing dir.
    Falls back to a literal one-segment step when nothing matches (e.g. the
    directory has since been deleted) — the resulting path string still hashes
    deterministically, it just may not match a live workspace.
    """
    segs = [s for s in project.split("-") if s != ""]
    cur = Path("/")
    i = 0
    while i < len(segs):
        matched = False
        for j in range(len(segs), i, -1):
            cand = "-".join(segs[i:j])
            if (cur / cand).is_dir():
                cur = cur / cand
                i = j
                matched = True
                break
        if not matched:
            cur = cur / segs[i]
            i += 1
    return str(cur)


@lru_cache(maxsize=None)
def workspace_id(cwd: str) -> int:
    out = subprocess.run(
        [WID_BIN, cwd], capture_output=True, text=True, check=True
    ).stdout.strip()
    return int(out)


def to_forge_ts(iso: str | None) -> str:
    """vault ISO8601 (…Z) -> diesel naive `YYYY-MM-DD HH:MM:SS.ffffff` (UTC)."""
    if not iso:
        dt = datetime.now(timezone.utc)
    else:
        try:
            dt = datetime.fromisoformat(iso.replace("Z", "+00:00"))
        except ValueError:
            dt = datetime.now(timezone.utc)
    dt = dt.astimezone(timezone.utc).replace(tzinfo=None)
    return dt.strftime("%Y-%m-%d %H:%M:%S.%f")


def build_context(session_id: str, rows: list[sqlite3.Row]) -> str:
    """Serialise vault messages into a forge `context` JSON blob."""
    messages = []
    for r in rows:
        role = {"user": "User", "assistant": "Assistant", "system": "System"}.get(
            r["role"].lower(), "User"
        )
        messages.append(
            {"message": {"text": {"role": role, "content": r["content"]}}}
        )
    return json.dumps({"conversation_id": session_id, "messages": messages})


def title_for(rows: list[sqlite3.Row]) -> str | None:
    for r in rows:
        if r["role"].lower() == "user":
            t = " ".join(r["content"].split())
            return (t[:70] + "…") if len(t) > 70 else t
    return None


def main() -> int:
    if not VAULT_DB.exists():
        log(f"vault db not found at {VAULT_DB} — nothing to import")
        return 0
    if not FORGE_DB.exists():
        log(f"forge db not found at {FORGE_DB} — run forge once first")
        return 0
    if not Path(WID_BIN).exists():
        log(f"helper binary not found at {WID_BIN} — run ./build.sh")
        return 1

    vault = sqlite3.connect(f"file:{VAULT_DB}?mode=ro", uri=True)
    vault.row_factory = sqlite3.Row
    forge = sqlite3.connect(str(FORGE_DB), timeout=30)
    forge.execute("PRAGMA busy_timeout=30000")

    existing = {r[0] for r in forge.execute("SELECT conversation_id FROM conversations")}
    sessions = vault.execute(
        "SELECT session_id, project, started_at FROM sessions ORDER BY started_at"
    ).fetchall()

    inserted = skipped = empty = 0
    for s in sessions:
        sid = s["session_id"]
        if sid in existing:
            skipped += 1
            continue
        rows = vault.execute(
            "SELECT role, content, timestamp FROM messages "
            "WHERE session_id=? ORDER BY id",
            (sid,),
        ).fetchall()
        if not rows:
            empty += 1
            continue

        cwd = resolve_cwd(s["project"])
        wid = workspace_id(cwd)
        ctx = build_context(sid, rows)
        created = to_forge_ts(s["started_at"] or rows[0]["timestamp"])
        updated = to_forge_ts(rows[-1]["timestamp"])
        forge.execute(
            "INSERT OR IGNORE INTO conversations "
            "(conversation_id, title, workspace_id, context, created_at, updated_at, metrics) "
            "VALUES (?,?,?,?,?,?,NULL)",
            (sid, title_for(rows), wid, ctx, created, updated),
        )
        inserted += 1

    forge.commit()
    forge.close()
    vault.close()
    log(f"inserted {inserted}, already-present {skipped}, empty-skipped {empty}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
