# Testing

## What to test

- **Test behavior, not tautologies (M-TAUTOLOGICAL-TESTS).** Don't restate a constant, mirror the implementation's branches, or assert what the type system already guarantees. Test the properties that matter:
  - Pairing: every old line and every new line appears in exactly one row, in order.
  - Folds: expanding every fold reproduces the original file.
  - Stats: `+`/`-` counts match the added/deleted rows.
- **Cover the edge cases that break diff viewers:**
  - empty files, and files without a trailing newline
  - CRLF line endings
  - binary files
  - non-UTF-8 paths and content
  - renames, and files deleted between status and read
  - a file entirely added or entirely deleted
  - very long lines
  - huge files (these are for performance tests, not unit tests)

## Where tests live

- **Unit tests** for private logic go in a `#[cfg(test)] mod tests` at the bottom of the same file.
- **Integration tests go under `crates/*/tests/` (M-INTEGRATION-TESTS).** That covers anything that only uses the public API. Prefer these when either kind would work.
- **Pure core steps** (diff → rows, folds) are tested with in-memory byte inputs. No repo or filesystem is needed, and that's the reason they're kept sans-IO.
- **Git-facing core code** (`repo`, `changes`, `content`) is tested against **real temporary repos**: a `tempfile` dev-dependency, `gix::init` or a `git` command to make commits, then assertions on the resulting `Changeset`. Don't mock gix. Put the shared repo-fixture helper in `crates/core/tests/common/mod.rs`.
- **TUI rendering** is tested with `ratatui::backend::TestBackend` and asserts on the buffer. Test `App::update` state transitions directly, without a terminal.
- **Doctests** on core entry points must pass (`cargo test --workspace` runs them).

## Test-only code (M-TEST-UTIL)

- Test helpers that must be shared across crates go behind a `test-util` Cargo feature. Never ship them in normal builds. Within one crate, `#[cfg(test)]` or `tests/common/` is enough.

## Lints in tests

- `clippy::unwrap_used` is on workspace-wide. To allow `unwrap()`/`expect()` in tests, set this in `clippy.toml` at the workspace root:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
```

  Don't scatter `#[allow(clippy::unwrap_used)]` on test modules.
- Panic messages in tests are optional (M-PANIC-MESSAGE exempts tests). Use `assert_eq!` over `assert!(a == b)` for better failure output.

## Running

```bash
cargo test --workspace                 # everything, incl. doctests
cargo test -p zdiff-core rows::        # one module's tests
cargo test -p zdiff-core -- --exact rows::tests::pairs_equal_hunks
```
