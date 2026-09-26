# Errors & panics

## Which error type where

- **`zdiff` (tui):** use `anyhow::Result` everywhere and add context with `.context("…")` / `.with_context(|| …)`. Don't mix in another app-level error type (M-APP-ERROR).
- **`zdiff-core`:** use a single `Error` type in `error.rs`, derived with `thiserror`. Callers in `tui` must be able to *react* to it (for example "not a git repository" vs "file vanished mid-refresh"), so the type has to carry that distinction. Don't return `String` or `Box<dyn Error>`.
  - Deviation from M-ERRORS-CANONICAL-STRUCTS: core isn't published and has one consumer, so a `thiserror` enum is fine, and we skip backtraces and `#[non_exhaustive]`. If core ever becomes a public library, switch to the canonical struct + private `ErrorKind` + `is_xxx()` methods form.
- **Convert with `From`, not scattered `map_err` (M-FROM-ERROR).** Use `#[from]` on variants so `?` works. Reserve `map_err` for when you're adding context the source error doesn't have, such as which path failed.

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not inside a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("failed to read {path}")]
    Read { path: PathBuf, #[source] source: std::io::Error },
    #[error(transparent)]
    Git(#[from] gix::open::Error),   // gix error stays private to core's API surface via Display/source
}
```

- Error messages are lowercase with no trailing period. They describe what failed, not what to do. Include the path or revision involved.
- Don't expose `gix` error types as part of a public signature beyond the `source()` chain (see api-design.md, M-DONT-LEAK-TYPES).

## Panics

- **A panic means "stop the program" (M-PANIC-IS-STOP).** Never use panics for control flow, to report errors upstream, or with the expectation that they'll be caught.
- **Bugs panic, runtime failures return errors (M-PANIC-ON-BUG).** An index past the end of a hunk you built yourself is a bug, so panic. A file deleted between status and read is a runtime condition, so return `Result` (and in practice, skip or refresh that file).
- **Prefer making the bad state unrepresentable** over checking it ("correct by construction"). Examples: a `LineNo` that can't be zero, or a row type that can't have both sides empty.
- **Valid reasons to panic:** a broken invariant (`expect("hunk ranges are sorted")`), const contexts, and a poisoned lock.
- **Every intentional panic has a message with the relevant values (M-PANIC-MESSAGE):**

```rust
assert!(
    row < self.rows.len(),
    "row {row} out of range for file with {} rows",
    self.rows.len()
);
```

- **Never `catch_unwind` to keep going (M-PANIC-CONTINUATION).** The only panic-related code in `tui` is the terminal-restore hook (`ratatui::init()` installs it). It must leave the terminal usable and then let the process exit.

## `unwrap` / `expect`

- `unwrap()` is linted (`clippy::unwrap_used`). Outside tests, use `?`, or `expect` with a message that states the invariant ("why this can't fail"), not the action ("failed to get row").
- In tests, `unwrap()` is fine (see testing.md for the lint config).

## Recoverable failures in the refresh loop

Transient errors during a live refresh, such as a file mid-write, a locked index, or a vanished path, must **not** kill the app. Keep the last good state for that file, show the error in the status bar, and retry on the next watcher event. Only startup failures (no repo, bad CLI args) should exit.
