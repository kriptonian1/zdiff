# Ownership & concurrency

## Ownership & borrowing

- **Borrow by default.** Take `&str`, `&[u8]`, `&Path` (or `impl AsRef<…>`, see api-design.md) when the function only reads. Take ownership only when the value is stored or moved to another thread.
- **Don't `clone()` to satisfy the borrow checker.** Restructure first: shorten the borrow, split the struct, or return an index or range instead of a reference. A `clone()` of file contents or row lists on the refresh path is a performance bug.
- **Return offsets or ranges into owned buffers** when references would tie lifetimes across the core → tui boundary awkwardly. Example: a `Line { range: Range<u32> }` into the file's byte buffer, not a `&'a str` that pins the whole changeset.
- **Add explicit lifetimes only where inference fails.** Don't put lifetime parameters on long-lived state types (`App`, `Changeset`). Those own their data.
- **No `Rc<RefCell<…>>` object graphs.** App state is a plain struct updated by `update(&mut self, action)`. If you need shared mutable state, question the design first.

## Concurrency model

zdiff has **no async runtime**. Don't add `tokio` or other async crates. The work is CPU-bound diffing plus file I/O, and `std` threads and channels handle it.

- **The UI thread** owns `App` state and the terminal. It blocks on a single `std::sync::mpsc::Receiver<Event>` that merges key input, watcher events, and finished-diff results.
- **The watcher** (`notify`) runs on its own thread and only *sends* debounced path sets. It never touches app state.
- **Diff work** runs off the UI thread when it could take noticeable time (large files, initial full status). Send results back as messages carrying owned data. Don't share state behind a lock.
- **Prefer message passing to shared locks.** If a lock is unavoidable, keep the critical section tiny and never hold it while doing I/O or diffing. A poisoned lock may be `expect`ed (see errors.md).
- **Stale results are expected.** Tag diff jobs with a generation counter per path. Drop a result whose generation is older than the file's current one, rather than cancelling threads.
- **`gix` handles:** `gix::Repository` isn't meant to be shared freely across threads. Use `ThreadSafeRepository` / `repo.into_sync()` and create a thread-local `Repository` in each worker with `.to_thread_local()`.

## Statics & globals (M-AVOID-STATICS)

- Don't use `static`/`thread_local!` for correctness-relevant state (config, caches the logic depends on). Pass it explicitly. Globals make tests order-dependent.
- Statics used purely as an optimization (for example a lazily built syntax set) are fine. Initialize them with `std::sync::LazyLock` / `OnceLock`, not `lazy_static`/`once_cell`.
