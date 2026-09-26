# API & module design

"API" here mostly means `zdiff-core`'s `pub` surface, which is what `tui` consumes. Keep it small and deliberate.

## The core ↔ tui boundary

- **Don't leak external types (M-DONT-LEAK-TYPES).** No `gix` type appears in any `pub` signature of `zdiff-core`. Convert to zdiff's own model types (`Changeset`, `FileDiff`, `FileStatus`, `Row`, …) at the edge of core. That keeps `tui` independent of gix and lets the backend be swapped.
- **Don't re-export foreign crates (M-FOREIGN-REEXPORTS).** No `pub use gix::…`. If `tui` needs something from a dependency, it depends on that crate directly, or core wraps it.
- **Put the change source behind a small trait,** for example `ChangeSource`. Keep it narrow: only the operations tui actually needs. One implementation (gix) is expected. Don't add generics or trait objects elsewhere "for flexibility".
- **Core is sans-UI and, where it's cheap, sans-IO (M-IMPL-IO).** Pure steps (line diff, row building, fold computation) take bytes or slices in and return data. They must not open files or repos themselves. That keeps them trivially testable.

## Module layout

- **Balanced modules (M-BALANCED-MODULES).** Put the handful of essential types in the crate root via `lib.rs` re-exports (`Changeset`, `FileDiff`, `Error`, …). Group the rest by domain (`repo`, `changes`, `diff`, `rows`), not by kind (no `types.rs`, `traits.rs`, `utils.rs`).
- **One path per item (M-SINGLE-ITEM-PATH).** If `lib.rs` re-exports `rows::Row`, make the `rows` module private (or `pub(crate)`) so `Row` isn't reachable as both `zdiff_core::Row` and `zdiff_core::rows::Row`.
- **No glob re-exports (M-NO-GLOB-REEXPORTS), no preludes (M-NO-PRELUDE).** Write `pub use rows::{Row, RowKind};`, never `pub use rows::*;`.
- **Don't add `pub` by default.** Items start private. Make them `pub(crate)` when shared inside the crate and `pub` only when `tui` needs them.

## Function signatures

- **Accept flexible borrowed inputs (M-IMPL-ASREF).** Use `impl AsRef<Path>`, `impl AsRef<str>`, or `impl AsRef<[u8]>` for read-only params in non-hot functions. In hot functions a concrete `&[u8]` / `&Path` is fine and avoids monomorphization bloat.
- **Don't infect types with those bounds.** `struct FileDiff { path: Box<Path> }`, not `struct FileDiff<P: AsRef<Path>>`.
- **Use ranges, not `(start, end)` pairs (M-IMPL-RANGEBOUNDS).** Take `Range<u32>` for line spans, or `impl RangeBounds<usize>` when callers benefit from `..`, `a..`, and similar.
- **Keep parameter order consistent (M-PARAMETER-CONSISTENCY).** The same conceptual params go in the same order everywhere (for example `repo`, then `path`, then `options`). Call-specific params come first, ubiquitous context last, and closures go at the very end.
- **Prefer free functions (M-REGULAR-FN).** A function that doesn't use a receiver is a module-level `fn`, not an associated fn on a random type. Associated fns are for constructors.
- **Essential behavior is inherent (M-ESSENTIAL-FN-INHERENT).** If a type implements a trait (like `ChangeSource`), the real logic lives in inherent methods and the trait impl forwards to them.

## Construction

- **0–2 optional settings:** `new()` plus `with_x()` methods. **3+ optional settings** (for example diff options: context size, whitespace mode, rename detection, …): a builder (M-INIT-BUILDER).
  - `Foo::builder()` returns `FooBuilder`, setters are named `x()` not `set_x()`, setters are infallible, and all validation happens in `.build() -> Result<Foo, Error>` (M-BUILD-RESULT).
- **Group related params (M-INIT-CASCADED).** If a function needs 4+ params, bundle them into a small struct such as `DiffOptions`. Don't write long positional argument lists.
- **Provide `Default`** where there's a sensible default (layout mode, diff options). Have `new()` alongside `Default` (C-CTOR).

## Keep it simple

- **No visibly nested generics in public types (M-SIMPLE-ABSTRACTIONS).** Callers should never write `Foo<Bar<Baz>>`.
- **No smart pointers in the API surface (M-AVOID-WRAPPERS).** Return `&T`, `&[T]`, or `T`, not `Arc<RefCell<T>>`. Use `Arc` internally when threads need it (see ownership-concurrency.md).
- **No speculative abstraction.** Don't add a trait, generic, or feature flag until a second real use exists (the one exception is the `ChangeSource` backend seam).
