# Contributing

Thanks for your interest in claude-forge! This is a small, focused Rust CLI.

## Before you start

Read [`PLAN.md`](PLAN.md) end to end — it is the authoritative spec. It documents
the Claude Code JSONL format, the Forge `context` format (mirrored from Forge's
own `conversation_record.rs`), the canonical model, the sync DB schema, and the
build milestones.

## Development

```sh
cargo build
cargo test
cargo fmt
cargo clippy --all-targets -- -D warnings
```

CI runs `fmt --check`, `clippy -D warnings` and `cargo test` on every push and
PR; please run them locally first.

## Working with real data — safely

- Never write to a live `~/.forge/.forge.db` while Forge is running, and never
  commit copies of personal conversations.
- Always test imports against a **copy** of the Forge DB, and run with
  `--dry-run` first.
- Integration tests that touch real data must be opt-in (gated), never run by
  default.

## Forge compatibility

The emitted `context` shapes target a specific Forge version (see the README's
"Works with Forge vX.Y.Z" line). If you bump it, re-read Forge's
`forge_repo/src/conversation/conversation_record.rs` and update the record
structs in `src/forge.rs` and the version note together.

## Commits & releases

Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `docs:`, `chore:`, …). Releases are managed by release-please and cut by
tagging `v*`, which triggers the cross-platform binary build.

By contributing you agree your contributions are licensed under the MIT license.
