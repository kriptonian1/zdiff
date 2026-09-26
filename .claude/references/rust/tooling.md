# Tooling & workspace

## Workspace layout (M-CARGO-WORKSPACE, M-CRATES-FLAT-FOLDER, M-CRATES-IN-WORKSPACE)

- There's one workspace `Cargo.toml` at the root. All crates are siblings under `crates/`, and none nests inside another crate. Related crates share a prefix (`zdiff-core`, `zdiff-…`).
- **Shared metadata comes from the workspace.** Every crate uses `version.workspace = true`, `edition.workspace = true`, and so on, plus `[lints] workspace = true`.
- **Dependency versions are declared once in `[workspace.dependencies]`** and crates reference them with `dep.workspace = true`. That includes deps only one crate uses. Workspace entries use `default-features = false` (except trivial ones like `std`), and each crate enables the features it needs:

```toml
# root Cargo.toml
[workspace.dependencies]
gix = { version = "0.88", default-features = false }

# crates/core/Cargo.toml
[dependencies]
gix = { workspace = true, features = ["basic", "status", "sha1", "parallel", "max-performance-safe"] }
```

- Intra-workspace deps go through `[workspace.dependencies]` (`zdiff-core.workspace = true`), never `path = "../core"` in a member crate.
- **Edition 2024 (M-LATEST-EDITION).** `rust-version` tracks the pinned toolchain in `rust-toolchain.toml` (M-MSRV). zdiff is an app, so bumping both together is fine.

## Adding dependencies

- Check `std` first. Every dependency costs build time and binary size, and sometimes resident memory.
- Add the dependency to the crate that uses it, keeping `gix` only in core and `ratatui`/`crossterm` only in tui. Enable the minimum features.
- Dev-only crates (`tempfile`, `insta`, `divan`) go in `[dev-dependencies]`.

## Crates & features (M-SMALLER-CRATES, M-FEATURES-ADDITIVE)

- Split a new crate out only when a part is independently usable or compile times demand it. For example, a syntax-highlighting layer could become `zdiff-highlight`. Don't split speculatively.
- **Cargo features must be additive.** Enabling a feature never removes or changes existing items. Every combination has to build.

## Lints

- **Current workspace lints:**
  - `unsafe_code = "forbid"`
  - clippy `all` + `pedantic` at warn
  - `unwrap_used` at warn
- **Overriding a lint locally:** use `#[expect(clippy::lint_name, reason = "…")]`, never a bare `#[allow]` (M-LINT-OVERRIDE-EXPECT). `#[expect]` warns when it becomes stale. `#[allow]` is acceptable only for generated code.
- **Candidates to add from M-STATIC-VERIFICATION:**
  - rust lints: `missing_debug_implementations`, `redundant_imports`, `redundant_lifetimes`, `trivial_numeric_casts`, `unused_lifetimes`
  - clippy lints: `allow_attributes_without_reason`, `clone_on_ref_ptr`, `if_then_some_else_none`, `map_err_ignore`, `redundant_type_annotations`, `semicolon_outside_block`, `unused_result_ok`, `too_long_first_doc_paragraph`

  Add them to `[workspace.lints]`, not to individual crates.

## Checks

These run locally before any change is considered done, and later as CI gates:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Periodic or pre-release checks:
- `cargo audit` (advisories)
- `cargo udeps` (unused deps, nightly)
- `cargo hack check --feature-powerset` (once features exist)

`miri` isn't needed while `unsafe` is forbidden.

## Profiles

- `release`: fat LTO, 1 codegen unit, `panic = "abort"`, stripped. Because of `panic = "abort"`, panics can't unwind in release, which is one more reason for M-PANIC-IS-STOP.
- `profiling`: release plus debug symbols, for profilers and flamegraphs.
