# claude-forge — implementation plan

> **For the implementer (a fresh AI or human).** This is the authoritative spec.
> Read it end to end before writing code. It assumes no prior context from the
> conversation that produced it. Where it says "verified", the fact was checked
> against live data / source on the author's machine on 2026-06-23.

## 1. What we are building

`claude-forge` is a single Rust binary that moves AI coding-agent conversations
between **Claude Code** and **Forge** (forgecode) **with full metadata** (tool
calls, tool results, token usage, reasoning, model id), and keeps a local
**sync-state database** so repeated runs are idempotent and, eventually,
bidirectional and conflict-aware.

It is the spiritual successor to two earlier tools that are now superseded:

- The Python **claude-vault** (`../claude-vault`) — archives Claude
  conversations into Obsidian markdown. We borrow its *packaging* (README style,
  CI/release automation, install ergonomics) and its *parsing know-how*, not its
  code. claude-vault is **lossy** (text only).
- The old **forge-vault-import** (this repo's git history) — a Python script
  that copied a lossy `vault.db` (sessions/messages text) into Forge, plus a
  std-only Rust helper to compute Forge's `workspace_id`. We **keep**
  `src/workspace_id.rs` (it is correct and tested) and **discard** everything
  that reads the lossy `vault.db`.

### Scope decisions (locked)

1. **v1 = direct Claude → Forge, full metadata.** Read Claude Code's *native*
   JSONL session files directly (not the lossy vault.db) and write rich Forge
   `context` rows that preserve tool calls, tool results, usage and reasoning.
2. **Build the sync-state DB now, designed for bidirectional**, but only wire
   the Claude→Forge direction in v1. Forge→Claude export is a later phase; the
   schema and canonical model must not need redesign to add it.
3. **DB design = canonical + raw-source + mapping.** Store a normalized
   canonical conversation, the *original raw native blob* per agent, and an
   id-mapping / content-hash table per agent. This is what "store the agent
   harness in the db to sync intelligently" means.
4. **Name = `claude-forge`** (binary `claude-forge`, cargo package
   `claude-forge`, GitHub repo `GabrielVidal1/claude-forge`). The architecture
   is agent-agnostic so more agents (OpenCode, etc.) can be added later, but the
   shipped name is `claude-forge`.

### Non-goals for v1

- No Obsidian/markdown export (that is claude-vault's job).
- No semantic search / embeddings / AI tagging.
- No live filesystem watching (a `sync` one-shot is enough; a `watch` mode can
  come later).
- Forge→Claude write-back is **planned but not implemented** in v1.

## 2. The three data models (verified)

### 2.1 Claude Code native JSONL (the v1 source)

Location: `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, one JSON
object per line. The directory name is the cwd with `/` and `.` replaced by `-`
(ambiguous to decode — but you do **not** need to decode it: each entry carries
the real `cwd`).

Entry `type` values seen in the wild: `user`, `assistant`, `attachment`,
`queue-operation`, `ai-title`, `last-prompt`, `file-history-snapshot`.

Top-level fields of interest: `uuid`, `parentUuid`, `sessionId`, `type`,
`timestamp` (ISO-8601, `…Z`), `cwd`, `gitBranch`, `version`, `requestId`,
`message`, `toolUseResult`, `isSidechain`, `isMeta`, `aiTitle`.

`message` (for user/assistant) holds: `role`, `content` (string **or** array of
blocks), and for assistant also `model`, `id`, `usage`, `stop_reason`.

Content block types: `text` (`{type:text,text}`), `thinking`
(`{type:thinking,thinking,signature}`), `tool_use`
(`{type:tool_use,id,name,input}`), `tool_result`
(`{type:tool_result,tool_use_id,content,is_error}`). Tool *results* arrive as a
`user`-role entry whose content array contains `tool_result` blocks; the
structured result is also mirrored in the top-level `toolUseResult`.

`usage` (assistant) has `input_tokens`, `output_tokens`,
`cache_creation_input_tokens`, `cache_read_input_tokens`, plus nested detail.

**Parsing rules borrowed from claude-vault** (`../claude-vault/claude_vault/code_parser.py`):
- Skip files named `history.jsonl` and `agent-*.jsonl` (subagent transcripts —
  their content is already embedded in the parent session as Task tool
  call/result; treating them as separate conversations causes UUID collisions).
- Skip entries with `type == "file-history-snapshot"`, `isMeta == true`, and
  string contents containing `<command-name>` / `<local-command-stdout>`.
- `sessionId` is the conversation id; `timestamp` ms-or-ISO both occur.

### 2.2 Forge database (the v1 target)

Location: `~/.forge/.forge.db` (SQLite). Table (verified):

```sql
CREATE TABLE conversations (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    title TEXT,
    workspace_id BIGINT NOT NULL,
    context TEXT,                  -- JSON: ContextRecord (see below)
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP,
    metrics TEXT                   -- JSON: MetricsRecord
);
CREATE INDEX idx_conversations_workspace_created ON conversations(workspace_id, created_at DESC);
CREATE INDEX idx_conversations_active_workspace_updated ON conversations(workspace_id, updated_at DESC) WHERE context IS NOT NULL;
```

Timestamps are diesel-naive UTC strings: `YYYY-MM-DD HH:MM:SS.ffffff`.

**`workspace_id`** = `std::hash::DefaultHasher` (SipHash-1-3, keys 0,0) over
`PathBuf::hash(cwd)`, stored as `i64`. Reproduced verbatim in
`src/workspace_id.rs` (keep it; it has unit tests against real Forge rows).
Because Claude JSONL carries the real `cwd`, v1 hashes that directly — **no
dash-decoding needed** (the old `resolve_cwd` heuristic is only needed when the
source lacks a real path; keep it available but unused for Claude import).

**The `context` JSON format is fully specified** by Forge's own
`../forgecode/crates/forge_repo/src/conversation/conversation_record.rs`. This
is the single most important reference — mirror it exactly. Key shapes:

```jsonc
// ContextRecord
{
  "conversation_id": "<uuid>",          // optional
  "messages": [ ContextMessageRecord ], // the transcript
  "tools": [...],                       // omit (empty)
  // tool_choice/max_tokens/temperature/top_p/top_k/reasoning/stream optional
}

// ContextMessageRecord  (note the `message` wrapper + optional usage)
{ "message": ContextMessageValueRecord, "usage": UsageRecord? }

// ContextMessageValueRecord  — serde rename_all="snake_case", externally tagged:
{ "text":  TextMessageRecord }   // a normal message
{ "tool":  ToolResultRecord }    // a tool result
{ "image": ImageRecord }

// TextMessageRecord
{
  "role": "System|User|Assistant",      // RoleRecord, PascalCase
  "content": "<string>",
  "tool_calls": [ ToolCallFullRecord ]?,// assistant tool invocations
  "model": "<model-id>"?,
  "reasoning_details": [ ReasoningFullRecord ]?,
  "thought_signature": "<str>"?,
  "raw_content": <json>?,
  "droppable": false                    // omitted when false
}

// ToolCallFullRecord
{ "name": "<tool>", "call_id": "<id>"?, "arguments": <json>, "thought_signature": <str>? }

// ToolResultRecord
{ "name": "<tool>", "call_id": "<id>"?,
  "output": { "is_error": bool, "values": [ ToolValueRecord ] } }

// ToolValueRecord — serde rename_all="camelCase", externally tagged:
{ "text": "<string>" } | { "image": ImageRecord } | "empty"
  // (also legacy: ai/markdown/fileDiff/pair — never emit these)

// UsageRecord (per-message, optional)
{ "prompt_tokens": TokenCount, "completion_tokens": TokenCount,
  "total_tokens": TokenCount, "cached_tokens": TokenCount, "cost": f64? }
// TokenCount = {"actual": n} | {"approx": n}   (rename_all camelCase)

// ReasoningFullRecord — all fields optional:
{ "text"?, "signature"?, "data"?, "id"?, "format"?, "index"?, "type_of"? }
```

`metrics` JSON (verified sample `{"started_at":"…Z","files_changed":{}}`):

```jsonc
// MetricsRecord
{ "started_at": "<rfc3339>"?, "files_changed": { "<path>": FileOp|[FileOp] },
  "files_accessed": ["<path>", ...] }   // files_accessed omitted when empty
// FileOp = { "lines_added": u64, "lines_removed": u64, "content_hash"?: str, "tool"?: ToolKind }
```

For v1 it is acceptable to emit a minimal metrics blob
(`{"started_at": <created_at>, "files_changed": {}}`) or `NULL`. Populating
`files_changed` from Edit/Write tool calls is a nice-to-have (Phase 4).

**Forge version compatibility:** the format above matches Forge **v2.13.14**
(latest local tag in `../forgecode`). The deserializer in
`conversation_record.rs` is intentionally lenient (untagged fallback for old
message shapes, optional everything), so emitted rows should also load in nearby
versions. Pin "Works with Forge v2.13.14" in the README and add a CI check that
re-reads `forge.schema.json` / the record file when bumping.

### 2.3 claude-vault state model (reference only)

`../claude-vault/claude_vault/state.py` keeps a `state.db` with
`conversations(uuid, file_path, content_hash, last_synced, metadata)` plus
embeddings/watch tables. We borrow the **content-hash + last-synced** idea for
idempotent sync, but our schema is richer (see §4).

## 3. The canonical model

Define an agent-neutral intermediate representation that both Claude and Forge
map to/from. This is what we normalize into and diff on.

```rust
struct CanonicalConversation {
    id: String,            // stable canonical id (see §4 mapping)
    title: Option<String>,
    cwd: Option<String>,   // drives Forge workspace_id; from Claude `cwd`
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    messages: Vec<CanonicalMessage>,
}

enum CanonicalMessage {
    Text {                 // user / assistant / system text turn
        role: Role,        // System | User | Assistant
        content: String,
        model: Option<String>,
        tool_calls: Vec<CanonicalToolCall>,   // assistant only
        reasoning: Vec<CanonicalReasoning>,    // thinking blocks
        usage: Option<Usage>,
        ts: DateTime<Utc>,
    },
    ToolResult {           // result of a tool call
        name: String,
        call_id: Option<String>,
        is_error: bool,
        values: Vec<ToolValue>,   // Text(String) | Image{..} | Empty
        ts: DateTime<Utc>,
    },
}
```

The canonical model is deliberately a **superset-lite**: it carries everything
Forge needs and everything Claude readily provides. Anything an agent can't
represent is preserved only in the stored raw blob (§4), not lost.

## 4. Sync-state database (canonical + raw + mapping)

A new SQLite DB owned by this tool. Default path
`~/.local/share/claude-forge/sync.db` (override `--state-db` / `CLAUDE_FORGE_DB`;
respect `$XDG_DATA_HOME`). Suggested schema:

```sql
-- one row per logical conversation (canonical identity)
CREATE TABLE conversations (
    canonical_id   TEXT PRIMARY KEY,     -- uuid we mint or adopt
    title          TEXT,
    cwd            TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    canonical_json TEXT NOT NULL,        -- serialized CanonicalConversation
    canonical_hash TEXT NOT NULL         -- sha256 of canonical_json
);

-- per-agent provenance: the "harness" we store to sync intelligently
CREATE TABLE agent_links (
    canonical_id   TEXT NOT NULL REFERENCES conversations(canonical_id),
    agent          TEXT NOT NULL,        -- 'claude' | 'forge'
    native_id      TEXT NOT NULL,        -- sessionId / conversation_id
    source_path    TEXT,                 -- jsonl path / db path
    raw_blob       TEXT NOT NULL,        -- ORIGINAL native bytes (jsonl text / forge context json)
    raw_hash       TEXT NOT NULL,        -- sha256 of raw_blob (change detection)
    last_seen      TEXT NOT NULL,        -- when we last read it from the agent
    last_written   TEXT,                 -- when we last wrote it to the agent
    PRIMARY KEY (agent, native_id)
);
CREATE INDEX idx_agent_links_canonical ON agent_links(canonical_id);
```

Why this shape:
- `raw_blob` per agent = the "agent harness" the user asked to keep. It lets us
  (a) re-export losslessly to the agent it came from, (b) detect upstream edits
  via `raw_hash`, and (c) debug mapping bugs against ground truth.
- `agent_links` maps one canonical conversation to its id in each agent — the
  basis for "don't re-import" and, later, bidirectional reconciliation.
- `canonical_hash` detects semantic changes independent of raw formatting.

**Idempotency for v1:** before writing a Claude session to Forge, upsert into
`conversations`/`agent_links`; skip the Forge write if a `forge` link already
exists for that `canonical_id` **and** the canonical hash is unchanged. Reuse
the Claude `sessionId` as the Forge `conversation_id` (verified: Forge accepts
arbitrary UUID strings as PK) so the mapping is stable and a session is inserted
at most once.

**Conflict handling (Phase 5, design now):** if both agents changed since
`last_seen` (both `raw_hash` differ from stored), flag a conflict; v1 resolution
policy = "source wins, log a warning". Keep last-writer metadata so a real
3-way merge can be added later.

## 5. CLI surface

```
claude-forge sync            # v1 default: Claude -> Forge, idempotent
    [--claude-dir ~/.claude] [--forge-db ~/.forge/.forge.db]
    [--state-db ~/.local/share/claude-forge/sync.db]
    [--since <date>] [--project <cwd-substr>] [--dry-run] [-v]

claude-forge status          # what's tracked, per-agent counts, drift
claude-forge workspace-id <path>   # keep: print Forge workspace_id for a cwd
# Phase 6+: claude-forge export --to claude <id>   (Forge -> Claude)
```

Use `clap` derive. Keep `workspace-id` (already implemented in `src/main.rs`).
Honor env overrides (`FORGE_DB`, `CLAUDE_FORGE_DB`). `--dry-run` must touch no
agent DB and no state DB writes that matter (report only).

Wire it to run before Forge starts via the bundled `forge` wrapper (already in
repo): it runs `claude-forge sync` best-effort, then `exec`s the real binary.

## 6. Mapping: Claude JSONL → Forge context (the heart of v1)

Per session file:
1. Stream lines; drop skipped types (§2.1). Keep `user`/`assistant` entries in
   file order (they are already causally ordered; `parentUuid` confirms).
2. For each kept entry build `CanonicalMessage`:
   - assistant `text` blocks → `Text{role:Assistant, content, model, usage}`.
   - assistant `thinking` blocks → `reasoning` entries
     (`ReasoningFullRecord{ text, signature }`).
   - assistant `tool_use` blocks → `tool_calls`
     (`ToolCallFullRecord{ name, call_id: id, arguments: input }`).
   - user `text`/string → `Text{role:User, content}`.
   - user `tool_result` blocks → `ToolResult{ name (from matching tool_use by
     tool_use_id), call_id: tool_use_id, is_error, values:[Text(stringified
     content)] }`. Match the name via a map of `tool_use_id -> name` built from
     preceding `tool_use` blocks.
   - Multiple text blocks in one message → join with `\n\n` (claude-vault does
     this); but keep tool_use as structured `tool_calls`, do **not** flatten
     them into the text (that was the old lossy behavior).
3. Convert canonical → `ContextRecord` (§2.2) and `serde_json::to_string`.
4. `workspace_id = workspace_id(cwd)` from the entry's `cwd`.
5. `created_at` = min ts, `updated_at` = max ts, both formatted as diesel-naive
   UTC micros (`to_rfc3339_opts(Micros,true)` then `T`→space, strip `Z`).
6. `title` = first user line, whitespace-collapsed, ≤70 chars + `…` (matches the
   old importer / Forge's own display expectations).
7. Upsert state DB; `INSERT OR IGNORE` into Forge `conversations`.

Edge cases to handle: empty sessions (skip), sessions whose `cwd` no longer
exists (still hash the string), assistant messages with only thinking/no text
(emit empty content + reasoning), interleaved tool_use/tool_result across
message boundaries, ms vs ISO timestamps, content as bare string vs array.

## 7. Crate layout

```
claude-forge/
  Cargo.toml                 # package "claude-forge", bin "claude-forge"
  src/
    main.rs                  # clap CLI + dispatch  (skeleton exists; expand)
    workspace_id.rs          # KEEP AS-IS (hashing + resolve_cwd, tested)
    canonical.rs             # CanonicalConversation/Message + (de)serialize
    claude/mod.rs            # native JSONL reader -> Canonical  (NEW, the work)
    forge/mod.rs             # Canonical -> ContextRecord + DB writer
    forge/record.rs          # serde structs mirroring conversation_record.rs
    state.rs                 # sync.db open/migrate/upsert/query
    sync.rs                  # orchestration: read claude, diff, write forge
  docs/ (this PLAN.md may live here or repo root)
  README.md, install.sh, forge (wrapper), .github/workflows/*
```

Dependencies: `clap` (derive,env), `rusqlite` (bundled), `serde`,
`serde_json`, `chrono` (clock), `dirs`, `sha2`, `anyhow`. (Current `Cargo.toml`
already has most; add `serde` derive and `sha2`.)

**Reuse from current scaffold:** `src/workspace_id.rs` verbatim; the clap
skeleton and Forge-write/timestamp helpers in the current `src/main.rs` /
`src/import.rs` as a starting point. **Discard** the vault.db (`sessions`/
`messages`) reading path entirely.

## 8. Packaging (make it usable by others, claude-vault-style)

- **README.md**: what it is; a prominent "Works with Forge **v2.13.14**" line;
  install (install.sh one-liner + `cargo install --git` + build from source);
  usage; the workspace_id/idempotency explanation; an **"Inspired by
  claude-vault"** section crediting `MarioPadilla/claude-vault` and a
  **"Using it alongside claude-vault"** section (claude-vault → Obsidian for
  human archive/search; claude-forge → Forge for resuming work; they read the
  same Claude source independently and don't conflict).
- **install.sh**: detect OS/arch, download the matching release asset from
  `GitHub releases` to `~/.local/bin/claude-forge`, `chmod +x`. Mirror the shape
  of typical Rust CLI installers.
- **CI/CD** under `.github/workflows/`:
  - `ci.yml`: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on
    push/PR (mirrors claude-vault's `ci.yml` intent, Rust toolchain).
  - `release.yml`: on tag `v*`, cross-compile a matrix
    (`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
    `aarch64-apple-darwin`, `x86_64-apple-darwin`) and upload binaries to the GH
    release.
  - `release-please.yml`: `release-type: rust` (mirrors claude-vault's
    automation) to manage version bumps + changelog.
- **LICENSE**: MIT. **CHANGELOG.md** + **CONTRIBUTING.md** (port claude-vault's
  CONTRIBUTING shape, adapted to cargo).

## 9. Testing

- Unit: `workspace_id` (keep existing tests); Claude block→canonical mapping
  with hand-written JSONL fixtures covering text/thinking/tool_use/tool_result;
  canonical→ContextRecord golden JSON compared against the shapes in §2.2.
- Round-trip sanity: emit a context row, then deserialize it with the same serde
  structs that mirror `conversation_record.rs` (and, ideally, a small Rust test
  that links `forge_domain` if feasible) to prove Forge will accept it.
- Integration (gated, opt-in): against copies of the real `~/.forge/.forge.db`
  and `~/.claude`, never the live files. Always run real imports with `--dry-run`
  first; **never** write to the live Forge DB while Forge is running.
- Idempotency: run `sync` twice; second run inserts 0.

## 10. Milestones

1. **M1 Skeleton**: rename done (`claude-forge`), `workspace-id` subcommand,
   `state.rs` with schema + migrations, `canonical.rs`.
2. **M2 Claude reader**: JSONL → Canonical with full metadata + fixtures/tests.
3. **M3 Forge writer**: Canonical → ContextRecord, DB upsert, idempotency via
   state DB; validate against a copy of the live DB with `--dry-run`.
4. **M4 Polish/metrics**: title/timestamps, optional `files_changed` metrics,
   `status` command, `forge` wrapper wiring.
5. **M5 Packaging**: README (+ Forge version + claude-vault sections), install.sh,
   CI/release workflows, LICENSE/CHANGELOG/CONTRIBUTING.
6. **M6 Publish**: create GitHub repo `GabrielVidal1/claude-forge`, push, cut a
   `v0.1.0` tag to trigger the release build.
7. **Later**: Forge→Claude export, conflict reconciliation, `watch` mode,
   additional agents (OpenCode).

## 11. Reference files (read these while implementing)

- Forge context spec (mirror exactly):
  `../forgecode/crates/forge_repo/src/conversation/conversation_record.rs`
- Forge schema: `../forgecode/crates/forge_repo/src/database/schema.rs`,
  migration `…/migrations/2025-09-12-065405_create_conversations_table/up.sql`
- Claude JSONL parsing know-how: `../claude-vault/claude_vault/code_parser.py`
- claude-vault sync/state ideas: `../claude-vault/claude_vault/state.py`,
  `sync.py`, packaging in `../claude-vault/.github/workflows/`
- Keep/extend: this repo's `src/workspace_id.rs` (tested), `forge` wrapper.

## 12. Watch-outs

- **Never edit the generated root `docker-compose.yml`** etc. — irrelevant here;
  this is a standalone project under `projects/` (gitignored by the homelab
  repo, its own git repo / GitHub remote).
- Forge must not be running during a write to `~/.forge/.forge.db`.
- The encoded project-dir name in `~/.claude/projects/` is **not** a reliable
  cwd source — use each entry's `cwd` field; only fall back to `resolve_cwd` if
  `cwd` is absent.
- Don't flatten `tool_use` into assistant text (the old lossy behavior we are
  explicitly replacing).
- Forge's deserializer tolerates missing/old fields, but **emit the modern
  shapes** (`{"message":{"text":{…}}}`, snake_case message tags, camelCase tool
  values, PascalCase roles) shown in §2.2.
```
