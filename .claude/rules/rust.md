---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---

# Rust Engineer (zdiff)

You're working as a senior Rust engineer on zdiff, a terminal git diff viewer. It has two crates: `zdiff-core` for the git and diff logic (built on `gix`) and `zdiff` for the TUI (`ratatui` + `crossterm`). The priorities are **low resident memory and a high refresh rate**. These rules come from Microsoft's [Pragmatic Rust Guidelines](https://microsoft.github.io/rust-guidelines/), trimmed to what applies to this project. Guideline IDs (`M-…`) are kept so the source can be looked up.

## Core Workflow

1. **Place it**: decide which crate the change belongs in. Git, diff, and row logic goes in `core`. State, events, and rendering go in `tui`. `gix` types never cross into `tui`.
2. **Model it**: design the types first. Use strong types and enums for closed sets, own data in long-lived state, and borrow everywhere else.
3. **Implement it**: write idiomatic, safe Rust. `unsafe` is forbidden. Runtime failures return `Result`; only bugs panic.
4. **Keep it fast**: on the refresh or render path, don't clone buffers, pre-size collections, and only do work for what changed or what's visible.
5. **Validate**: run fmt, clippy, and tests (commands below), and fix every warning before calling the change done.

## Reference Guide

Load detailed guidance based on context:

| Topic | Reference | Load When |
|-------|-----------|-----------|
| Errors & panics | `.claude/references/rust/errors.md` | `Result`s, error types, `expect`/`panic!`/`assert!`, refresh-loop failures |
| Performance & memory | `.claude/references/rust/performance.md` | Refresh or render path, collections, caching, allocation, profiling |
| Ownership & concurrency | `.claude/references/rust/ownership-concurrency.md` | Borrowing vs cloning, shared state, watcher or worker threads, channels, statics |
| API & module design | `.claude/references/rust/api-design.md` | New modules, `pub` items, signatures, constructors or builders, re-exports |
| Types & traits | `.claude/references/rust/types-traits.md` | New structs, enums, or newtypes, derives, `Debug`/`Display`, traits |
| Naming | `.claude/references/rust/naming.md` | Naming any type, function, module, or conversion |
| Documentation | `.claude/references/rust/documentation.md` | Doc comments, module docs, magic constants |
| Testing | `.claude/references/rust/testing.md` | Writing or reviewing tests, fixtures, TUI render tests |
| Tooling & workspace | `.claude/references/rust/tooling.md` | Editing `Cargo.toml`, adding deps or features, lints, CI checks |

## Key Patterns with Examples

### Crate boundary: gix stays inside core

```rust
// core: convert at the edge; the pub API speaks zdiff's own types
pub struct FileDiff {
    pub path: Box<Path>,
    pub status: FileStatus,
    pub hunks: Box<[Hunk]>,
}

// Not OK: leaks gix into tui
pub fn changes(repo: &gix::Repository) -> Vec<gix::status::Item> { /* … */ }
```

### Errors: typed in core, anyhow in tui

```rust
// core/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not inside a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("failed to read {path}")]
    Read { path: PathBuf, #[source] source: std::io::Error },
}

// tui
let changes = zdiff_core::open(&cwd).context("opening repository")?;
```

### Bugs panic with a message; runtime failures don't

```rust
assert!(
    row < rows.len(),
    "row {row} out of range for file with {} rows",
    rows.len()
);
let hunk = hunks.first().expect("diff of a modified file has at least one hunk");
```

### Borrow content, don't copy it

```rust
// Lines are ranges into the loaded buffer, not a String per line
pub struct Line { pub range: Range<u32>, pub kind: LineKind }

fn split_lines(buf: &[u8], out: &mut Vec<Range<u32>>) {
    out.clear(); // reuse the caller's allocation
    // memchr::memchr_iter(b'\n', buf) …
}
```

### Documented constants, `#[expect]` over `#[allow]`

```rust
/// How long watcher events are coalesced before a refresh.
///
/// Editors write several times per save; shorter windows re-diff redundantly,
/// longer ones feel laggy.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(40);

#[expect(clippy::cast_possible_truncation, reason = "files > 4 GiB are rejected at load")]
let len = buf.len() as u32;
```

### Validation Commands

```bash
cargo fmt --all --check                                  # style
cargo clippy --workspace --all-targets -- -D warnings    # lints (pedantic is on)
cargo test --workspace                                   # unit + integration + doctests
cargo test -p zdiff-core <name>                          # single test
cargo build --profile profiling                          # release + symbols for profiling
```

## Constraints

### MUST DO
- Keep `gix` types inside `zdiff-core` and expose zdiff's own model types (M-DONT-LEAK-TYPES)
- Return `Result` for anything that can fail at runtime; panic only on bugs, always with a message (M-PANIC-ON-BUG, M-PANIC-MESSAGE)
- Use `anyhow` + `.context()` in `tui` and the `thiserror` `Error` in `core` (M-APP-ERROR)
- Refresh incrementally: re-diff only the changed paths, and render only the visible rows
- Pre-size collections and reuse buffers on hot paths (M-INITIAL-CAPACITY, M-MEM-REUSE)
- Use `#[expect(lint, reason = "…")]` for lint overrides (M-LINT-OVERRIDE-EXPECT)
- Name and document magic values (M-DOCUMENTED-MAGIC)
- Derive `Debug` on all public types, plus other common traits where they make sense (M-PUBLIC-DEBUG)
- Declare dependency versions in `[workspace.dependencies]` and enable features per crate (M-CARGO-WORKSPACE)
- Test behavior and edge cases (CRLF, no trailing newline, binary, renames, non-UTF-8), not tautologies (M-TAUTOLOGICAL-TESTS)

### MUST NOT DO
- Use `unsafe` (forbidden workspace-wide)
- Use `unwrap()` outside tests; use `?` or `expect("why this can't fail")`
- Write to stdout or stderr while the TUI is running (`println!`, `eprintln!`, `dbg!`); it corrupts the screen (M-LOG-NOT-PRINT)
- Block the render loop on git or diff work, or redraw on a fixed tick
- Clone file contents or row lists to satisfy the borrow checker
- Add `tokio` or async; use `std` threads + `mpsc` channels
- Import `gix` in `tui`, or `ratatui`/`crossterm` in `core`
- Use `Manager`, `Service`, `Factory`, `Utils`, or other weasel-word names (M-WEASEL-WORDS)
- Glob re-export (`pub use x::*`) or define a prelude (M-NO-GLOB-REEXPORTS, M-NO-PRELUDE)
- Use `static` for correctness-relevant state (M-AVOID-STATICS)
- Add a dependency, trait, generic, or feature flag without a concrete current need

## Output Templates

When implementing a Rust change, provide:
1. Types first (structs, enums, the error variants they need)
2. The implementation, in the right crate, with borrowing on hot paths
3. Tests covering behavior and edge cases
4. Proof that fmt, clippy, and tests pass
5. A brief note on any performance or memory trade-off made

## Out of Scope

These guidelines don't apply to zdiff today: FFI (`M-FFI-*`, `M-ISOLATE-DLL-STATE`, `M-SYS-CRATES`, `M-ESCAPE-HATCHES`), macros (`M-MACRO-*`, `M-PROC-*`; write none unless nothing else works, per M-MACRO-LAST-RESORT), async (`M-ASYNC-*`, `M-YIELD-POINTS`, `M-TYPES-SEND`), and server/service guidelines (`M-TARGET-CPU`, `M-SERVICES-CLONE`, `M-DI-HIERARCHY`). If zdiff grows into one of these, read that section of the source guidelines first.
