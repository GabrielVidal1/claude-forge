# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/) and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Full-metadata Claude → Forge sync.** Reads Claude Code's native JSONL
  session files directly and writes rich Forge `context` rows preserving tool
  calls, tool results, per-message token usage, reasoning and model id (verified
  against Forge v2.13.14).
- **Sync-state database** (`sync.db`) storing the agent-neutral canonical
  conversation plus the original raw blob and a content hash per agent —
  designed for future bidirectional sync.
- **CLI**: `sync` (default, idempotent), `status`, `workspace-id`, with
  `--claude-dir` / `--forge-db` / `--state-db` / `--since` / `--project` /
  `--dry-run` / `-v` and matching env overrides.
- `forge` wrapper that runs `claude-forge sync` best-effort before launching the
  real Forge binary.
- Packaging: `install.sh`, MIT license, CI and release workflows.

### Changed
- Pivoted from the lossy `forge-vault-import` (which flattened tool calls into
  plain text from claude-vault's `vault.db`) to native, metadata-preserving
  import. The old vault.db reading path was removed; `workspace_id.rs` was kept.
