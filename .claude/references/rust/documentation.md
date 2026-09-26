# Documentation

zdiff-core isn't published, so documentation is for the next person (or agent) reading the code. Be thorough where behavior isn't obvious and silent where it is.

## Doc comments

- **Every `pub` item in `zdiff-core` gets a `///` doc.** Private items get one only when their behavior isn't obvious from the name and signature.
- **The first sentence is one line, about 15 words or fewer (M-FIRST-DOC-SENTENCE).** It's the summary shown in module listings. Put details in later paragraphs.
- **Use canonical sections where they apply (M-CANONICAL-DOCS):**

```rust
/// Builds side-by-side rows for one file's hunks.
///
/// Deletions and additions in the same hunk are paired line by line; the
/// shorter side is padded with filler rows. Unchanged runs longer than
/// `fold_threshold` collapse into a single fold row.
///
/// # Errors
/// Returns [`Error::Binary`] if either side is binary.
///
/// # Panics
/// Panics if hunk ranges are not sorted (a bug in the diff step).
pub fn build_rows(/* … */) -> Result<Rows, Error> { /* … */ }
```

  - `# Errors` is required on any `pub fn` returning `Result`. `# Panics` is required if it can panic. There's no `# Safety` section because unsafe is forbidden.
  - `# Examples` is encouraged for core entry points. Examples are doctests and must compile and pass.
- **Describe parameters in prose,** for example "Diffs `old` against `new` …". Don't write `# Parameters` tables.
- Link related items with intra-doc links (`[`Changeset`]`, `[`Row::kind`]`).

## Module docs (M-MODULE-DOCS)

- Each module in core starts with a `//!` doc: what the module does, where it sits in the pipeline (repo → changes → content → diff → rows), and any guarantees (for example "rows are ordered by new-side line number"). Keep the first sentence short.

## Re-exports (M-DOC-INLINE)

- Crate-root re-exports of our own items use `#[doc(inline)]` so they render as first-class items.

## Magic values (M-DOCUMENTED-MAGIC)

- **No unexplained literals in logic.** Give each one a named `const` with a doc comment covering why this value, what changes if you tune it, and what it interacts with.

```rust
/// How long file-watcher events are coalesced before a refresh.
///
/// Editors often write a file several times per save; shorter windows cause
/// redundant re-diffs, longer ones make the view feel laggy.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(40);
```

## What not to write

- **No meta or design narrative in docs or comments (M-NO-META-DESIGN-DOCUMENTATION).** No "we chose X because in an earlier version…", no "following guideline M-…", and no changelog-style comments. Document what the code does now. Architecture rationale belongs in CLAUDE.md or a README.
- Don't write comments that restate the code (`// increment i`).
