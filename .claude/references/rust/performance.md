# Performance & memory

zdiff is performance-critical by design: **low resident memory and a high refresh rate**. Every change on the refresh/render path should keep both in mind.

## Architecture-level rules (these matter most)

- **Incremental refresh.** A file-watcher event causes a re-read and re-diff of *only the changed paths*. Run a full status only at startup or when `.git/index` / `HEAD` changes.
- **Cache the old side.** HEAD/index blob contents don't change while the user edits, so load them once per file and reuse them.
- **Bound what's resident.** Keep full contents and rows only for the selected file and a few nearby ones. For everything else keep path, status, and `+/-` stats. Evict when the selection moves.
- **Render only the viewport.** Build or slice rows for the visible range. Never format all rows of a 10k-line file each frame.
- **Redraw on change, not on a tick.** The event loop blocks on input and watcher events. No busy loop or fixed-interval redraw.
- **Batch and debounce (M-THROUGHPUT).** Coalesce bursts of watcher events (editors write several times per save) into one refresh of the union of paths. Never spin waiting for work.

## Allocation

- **Collections with a known size use `with_capacity`, or `collect()` from a sized iterator (M-INITIAL-CAPACITY).** Example: `Vec::with_capacity(hunk.len())` for rows.
- **Reuse buffers in hot loops (M-MEM-REUSE).** Keep a scratch `String`/`Vec` on the struct and `.clear()` it instead of allocating per line or per frame. Design core functions so callers can pass a buffer in (`fn build_rows_into(&self, out: &mut Vec<Row>)`) where it's on the hot path.
- **Shrink long-lived collections (M-SHRINK-TO-FIT).** Call `shrink_to_fit()` on large collections that were grown incrementally and then kept, such as a file list built by pushing.
- **Use boxed immutable sequences for many-instance data (M-BOX-DST).** Once built, paths and other immutable strings stored per file can be `Box<str>` / `Arc<str>` instead of `String`, and fixed row lists `Box<[Row]>`. This drops the capacity word and any excess capacity.
- **Borrow, don't copy, file contents.** Lines should be `&[u8]` / `&str` slices or `Range<usize>` offsets into the loaded buffer, not one `String` per line. Use `memchr` to find newlines.
- **Don't clone file contents or row vectors to hand them to the UI.** Pass references, or share with `Arc` when a worker thread produced them.

## Data layout

- **Avoid needless indirection (M-AVOID-INDIRECTION).** Keep hot fields inline. Don't nest `Arc<Config { Arc<Theme> }>`. Copy small flags (layout mode, "is binary") next to where the render loop reads them.
- **Keep row types small.** A `Row` is created per visible line. Prefer `u32` line numbers and a small `enum` kind over `usize`s plus `Option<String>`s.
- **Use a fast hasher for internal keys (M-FAST-HASHER).** Path → file maps are trusted data, so `foldhash`/`FxHash` is appropriate *once a profile shows hashing matters*. Don't add the dependency speculatively.

## Measure, don't guess (M-HOTPATH)

- Hot paths are status, blob load, line diff, row building, and frame render. Add benchmarks (`divan` or `criterion`) when optimizing one of them, and keep them.
- Profile with the `profiling` Cargo profile (`cargo build --profile profiling`), which has release optimizations plus debug symbols.
- Common culprits: repeated reallocation of growing `String`s/`Vec`s, cloning strings/collections, short-lived per-line allocations, and re-hashing.
- **Allocator (M-MIMALLOC-APPS):** the guidelines recommend `mimalloc` for apps for speed. For zdiff, only adopt it if a benchmark shows a real refresh-time gain *and* resident memory doesn't regress. Memory is a primary goal here.
