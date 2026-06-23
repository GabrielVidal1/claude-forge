# claude-forge

> 🚧 **Work in progress.** A single Rust binary that moves AI coding-agent
> conversations between **Claude Code** and **Forge** (forgecode) with full
> metadata, backed by a local sync-state database.

Successor to the lossy `forge-vault-import` script and inspired by
[claude-vault](https://github.com/MarioPadilla/claude-vault).

**The full design and implementation spec lives in [`PLAN.md`](PLAN.md).** Read
it before contributing — it documents the Claude JSONL and Forge `context`
formats, the canonical model, the sync DB schema, and the build milestones.

Currently implemented: `claude-forge workspace-id <path>` (prints the Forge
`workspace_id` for a directory). Everything else is per the plan.
