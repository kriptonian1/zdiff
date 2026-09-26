# Types & traits

## Strong types (M-STRONG-TYPES, M-STRONG-TYPES-GUARD)

- **Use the strongest fitting `std` type as early as possible.** Paths are `Path`/`PathBuf` (or `Box<Path>`), not `String`. Git paths that may not be UTF-8 stay as bytes (`BString` inside core, converted at the edge) and are only lossily converted to `str` for display.
- **Replace primitives where mixing them up is a real risk.** Old-side and new-side line numbers are both `u32` and easy to swap. Consider `OldLine(u32)` / `NewLine(u32)`, or a `Side` enum, in row-building code.
- **A newtype that encodes an invariant enforces it.** Construction is fallible (`TryFrom` / `fn new(..) -> Result`) or `const`-panicking. There is no infallible `From` from the weaker type. Don't make callers uphold the invariant.
- **Use enums for closed sets:** `FileStatus { Added, Modified, Deleted, Renamed { from } , … }`, `RowKind`, `LayoutMode { Split, Stacked }`. Not bools or strings. Replace two bools that are never both true with an enum.
- **Use regular numeric types at boundaries.** Don't expose `Saturating<…>`/`Wrapping<…>` in signatures.

## Derives & common traits (C-COMMON-TRAITS)

- **Every public type implements `Debug` (M-PUBLIC-DEBUG).** Normally `#[derive(Debug)]`. For types holding large buffers (file contents), write a manual `Debug` that prints lengths or ids, not megabytes of bytes.
- Eagerly derive `Clone`, `Copy` (small plain data like `RowKind`, `LayoutMode`, line numbers), `PartialEq`/`Eq`, `Hash`, and `Default` where they make sense. **Exception:** don't derive `Clone` on types holding whole-file buffers unless it's needed. An accidental clone there is a memory regression.
- **Implement `Display` for user-readable types (M-PUBLIC-DISPLAY):** errors (via `thiserror`), `FileStatus` (renders `M`/`A`/`D`/`R`), and path wrappers. Follow Rust conventions: no trailing newline.

## Traits

- **Keep traits narrow and purposeful.** The planned one is the `ChangeSource` backend seam. Don't create traits for single implementations "for testability". Test pure functions directly, and test gix code against real temp repos.
- **Prefer, in order:** concrete types, then generics (`impl Trait`), then `dyn Trait`. Use `dyn` only where heterogeneous collections or compile-time costs genuinely require it.
- **Implement std conversion traits** (`From`, `TryFrom`, `AsRef`, `IntoIterator`) instead of ad-hoc `to_x()`/`from_x()` methods when the conversion is the canonical one.
- **Custom collections implement iterator traits (M-COLLECTION-TRAITS).** If you create a collection type (for example `Rows`), implement `iter()`, `IntoIterator` for `&Rows`, and an honest `size_hint`/`ExactSizeIterator`, so callers can use `for` loops and `collect`.

## Globals

See ownership-concurrency.md (M-AVOID-STATICS): no correctness-relevant `static` state; `LazyLock`/`OnceLock` only for pure optimization.
