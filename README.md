# claude-forge

A single Rust binary that moves AI coding-agent conversations from
**Claude Code** into **[Forge](https://github.com/antinomyhq/forge)** (forgecode)
**with full metadata** — tool calls, tool results, token usage, reasoning and
model id — backed by a local sync-state database so repeated runs are
idempotent.

> **Works with Forge v2.13.14.** The emitted `context` rows mirror Forge's own
> `conversation_record.rs`. Forge's deserializer is lenient (optional fields,
> untagged fallbacks), so rows should also load in nearby versions.

It is the successor to the lossy `forge-vault-import` script and is inspired by
[claude-vault](https://github.com/MarioPadilla/claude-vault). Where the old
importer flattened tool calls into plain text, claude-forge reads Claude Code's
**native JSONL** session files directly and preserves the structured transcript.

## What it does

After every Forge launch (or whenever you run it), claude-forge:

1. Reads Claude Code's native session files under `~/.claude/projects/`.
2. Normalizes each into an agent-neutral canonical conversation (preserving
   tool calls, tool results, per-message usage and reasoning).
3. Writes any **new** conversations into Forge's SQLite DB as rich `context`
   rows, computing the correct `workspace_id` so they appear in the right Forge
   workspace.
4. Records everything in a local `sync.db` (canonical form + the original raw
   blob per agent) so it never re-imports and is ready for future bidirectional
   sync.

Because it reuses the Claude `sessionId` as the Forge `conversation_id` and
inserts with `INSERT OR IGNORE`, **a session is written at most once** — the tool
is safe to run on every launch.

## Install

One-liner (downloads the matching release binary to `~/.local/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/GabrielVidal1/claude-forge/main/install.sh | sh
```

With cargo:

```sh
cargo install --git https://github.com/GabrielVidal1/claude-forge
```

From source:

```sh
git clone https://github.com/GabrielVidal1/claude-forge
cd claude-forge
cargo build --release
# binary at target/release/claude-forge
```

## Usage

```sh
claude-forge sync                 # default: Claude -> Forge, idempotent
claude-forge sync --dry-run -v    # show what would be inserted, write nothing
claude-forge sync --since 2026-06-01 --project zipgo
claude-forge status               # what's tracked + Forge row count
claude-forge workspace-id <path>  # print the Forge workspace_id for a directory
```

Paths and overrides (all have sensible defaults):

| Flag / env | Default |
| --- | --- |
| `--claude-dir` / `CLAUDE_DIR` | `~/.claude` |
| `--forge-db` / `FORGE_DB` | `~/.forge/.forge.db` |
| `--state-db` / `CLAUDE_FORGE_DB` | `$XDG_DATA_HOME/claude-forge/sync.db` |

`--dry-run` writes to **no** database (neither Forge nor the state DB inserts are
committed). Always run a dry-run first when pointing at a real Forge DB, and
**do not** sync into a Forge DB while Forge itself is writing to it.

### Run it automatically before Forge

The bundled [`forge`](./forge) wrapper runs `claude-forge sync` best-effort and
then `exec`s the real Forge binary. Put it earlier on `PATH` than the real
binary (or rename the real one to `forge-bin`):

```sh
ln -sf "$PWD/forge" ~/.local/bin/forge
mv "$(which forge)" ~/.local/bin/forge-bin   # if needed; or set REAL_FORGE
```

## How the workspace_id works

A Forge conversation only shows up in a workspace if its `workspace_id` matches
Forge's hash of that workspace's directory. Forge derives it by hashing the
`PathBuf` with the std `DefaultHasher` (SipHash-1-3, keys 0,0) and storing
`hash as i64`. claude-forge reproduces this exactly in `src/workspace_id.rs`
(unit-tested against real Forge rows), hashing the real `cwd` carried by each
Claude entry — no dash-decoding needed.

## Using it alongside claude-vault

The two tools read the same Claude source independently and don't conflict:

- **[claude-vault](https://github.com/MarioPadilla/claude-vault)** → Obsidian
  markdown, for a searchable human archive (text only).
- **claude-forge** → Forge `context` rows, for *resuming work* in Forge with the
  full structured transcript.

## Status

v1 is one-directional (Claude → Forge). The sync DB schema and canonical model
are designed for bidirectional sync; Forge → Claude export, conflict
reconciliation, a `watch` mode and additional agents (OpenCode, …) are planned.

The full design and the data-format references live in [`PLAN.md`](PLAN.md).

## License

MIT — see [LICENSE](LICENSE).
