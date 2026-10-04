# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

zdiff is a terminal git diff viewer (side-by-side and stacked views, file sidebar, foldable unchanged regions) for watching your working-tree changes live. Design goals: **low memory and a high refresh rate**.

The project is at the scaffold stage: `crates/core/src/lib.rs` and `crates/tui/src/main.rs` are still `cargo new` stubs. The architecture below is the agreed design to build toward.

## Commands

The toolchain is pinned to Rust 1.98.1 (edition 2024) via `rust-toolchain.toml`.

```bash
cargo build                         # builds the default member (tui → `zdiff` binary)
cargo run -- <args>                 # run the viewer
cargo build --workspace             # build all crates
cargo test --workspace              # all tests
cargo test -p zdiff-core <name>     # single test (substring match) in core
cargo clippy --workspace --all-targets
cargo fmt --all
cargo build --profile profiling     # release optimizations + debug symbols, for profilers
cargo build --profile dist          # fat LTO, what the release workflow ships (slow)
```

`default-members = ["crates/tui"]`, so a bare `cargo test` / `cargo build` only covers the tui crate — pass `--workspace` or `-p zdiff-core` for core.

## Lints

Workspace lints apply to both crates: `unsafe_code = "forbid"`, clippy `all` + `pedantic` at warn, and `unwrap_used` warn. Code should pass clippy clean; don't use `unwrap()` outside tests (use `?`, `expect` with a reason, or proper error handling).

## Architecture

Cargo workspace with two crates and a strict boundary:

- **`crates/core` (`zdiff-core`)** — all git and diff logic. Uses `gix` (in-process, no shelling out to `git`) with default features off (`basic, status, sha1, parallel, max-performance-safe`). Typed errors via `thiserror`. **Must never depend on `ratatui`/`crossterm`.**
- **`crates/tui` (`zdiff` binary)** — CLI args (`clap` derive), terminal event loop (`crossterm`), rendering (`ratatui`), top-level errors via `anyhow`. **Never touches `gix` or computes diffs itself.** It only consumes core's model/row types.

Intended data flow:

1. **repo** — discover/open the repository, resolve revisions.
2. **changes** — list changed paths and their status (added/modified/deleted/renamed) for the chosen mode: unstaged (index vs worktree), staged (HEAD vs index), vs HEAD, or a rev range (tree vs tree).
3. **content** — load old/new bytes per file (object DB blob or worktree file), detect binary.
4. **diff** — line diff of the two versions via `gix::diff::blob` (imara-diff) → hunks. `memchr` for line splitting. No unified-diff text is generated or parsed, except `core/src/patch.rs`, which reads patch files for `--patch`: a patch has only hunks, so it yields a partial `FileDiff` (`known` ranges, `Row::Gap` between them) and never touches a repository.
5. **rows** — build display-ready rows: pair deletions with additions for the split view, add filler rows, mark foldable unchanged regions. This is pure and unit-testable, and it lives in core, not tui.
6. **tui** — app state (selection, scroll, focus, layout mode) → render.

gix types stay inside core; expose zdiff's own model types. Keep the change source behind a small trait so another backend (e.g. the git CLI) could be added without touching tui.

### Performance design (the reason for choosing gix)

- Refresh incrementally: a file watcher with debouncing triggers a re-diff of **only the changed paths**. Run a full status only at startup or when `.git/index` / `HEAD` changes.
- Cache old-side blobs (HEAD/index content rarely changes while editing).
- Keep full contents only for the selected file and a few nearby ones. Keep only stats for the rest.
- Build and render rows only for the visible viewport. Redraw on state changes, not on a fixed tick.
