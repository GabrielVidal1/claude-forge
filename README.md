# forge-vault-import

Import Claude Code conversations archived by **claude-vault** into the
**forgecode** database, so past Claude Code chats appear in forge's history.

Designed to run **before forge starts** — it's idempotent, so it's safe to run
on every launch.

## How it works

- **claude-vault** stores Claude Code sessions in `~/.local/share/claude-vault/vault.db`
  (one row per message: `session_id, role, content, timestamp`).
- **forgecode** stores each conversation as a single row in
  `~/.forge/.forge.db` → `conversations`, with the whole transcript serialised
  as a JSON `context` blob.

`import.py` copies every vault session forge doesn't already have. The vault
`session_id` (a UUID) is reused as the forge `conversation_id`, so each session
is imported at most once.

### The `workspace_id` problem

A forge conversation only shows up in a workspace if its `workspace_id` equals
forge's hash of that workspace's cwd: `DefaultHasher` (SipHash-1-3, keys 0,0)
over `PathBuf::hash`. That hash is awkward to reproduce by hand (`Path::hash`
normalises separators), so we shell out to **`forge-workspace-id`** — a tiny
std-only Rust binary (`workspace_id.rs`) that calls the exact std
implementation forge uses. Validated against real forge rows:

| cwd | workspace_id |
|---|---|
| `/home/gabrielvidal` | `-8599109238221935417` |
| `/home/gabrielvidal/homelab` | `8968329562854484240` |
| `/home/gabrielvidal/homelab/projects/zipgo` | `-3877205949088219147` |

The cwd is reconstructed from vault's dash-encoded `project` name
(`-home-gabrielvidal-homelab`) by greedily matching against the real
filesystem.

### Lossiness

vault only keeps user/assistant **text** — tool calls are flattened into the
assistant content string (`[tool_use: Bash] {…}`), and there are no usage or
tool-result records. Imported conversations are therefore readable plain-text
transcripts, not byte-perfect forge contexts.

## Setup

```bash
./build.sh                 # compiles forge-workspace-id -> ~/.local/bin/
```

Requires `rustc` (rustup) once to build the helper, and `python3` (stdlib only)
to run the importer.

## Usage

```bash
python3 import.py          # import any new sessions into ~/.forge/.forge.db
```

> Run it only while **forge is not running** (it writes to forge's live DB).

Env overrides: `VAULT_DB`, `FORGE_DB`, `FORGE_WORKSPACE_ID`.

## Wiring it to run before forge

Use the bundled `forge` wrapper — it runs the import (best-effort, never blocks)
then `exec`s the real binary. One-time install, e.g.:

```bash
mv ~/.local/bin/forge ~/.local/bin/forge-bin       # the real binary
ln -sf "$PWD/forge" ~/.local/bin/forge             # wrapper takes its place
```

The wrapper finds the real binary at `~/.local/bin/forge-bin` (override with
`REAL_FORGE`).
