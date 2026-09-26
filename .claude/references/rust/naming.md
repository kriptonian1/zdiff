# Naming

Follow the [Rust API Guidelines naming section](https://rust-lang.github.io/api-guidelines/naming.html), plus the rules below.

## Short names (M-SHORT-NAMES)

- Compound at most **two** short words: `DiffOpts`/`DiffOptions`, not `UnifiedDiffComputationOptions`.
- **Don't repeat the module or crate in the name.** Users disambiguate with the path. Write `rows::Row`, not `rows::DiffRow`, and `changes::Status`, not `changes::ChangeStatus`, unless two same-named types would genuinely meet in one scope.
- Established abbreviations are fine and preferred: `Opts`, `Ctx`, `Fn`, `Idx`, `Rev`, `Oid`.

## No weasel words (M-WEASEL-WORDS)

- **Banned in type names:** `Manager`, `Service`, `Factory`, `Handler`, `Helper`, `Util(s)`, `Data`, `Info`, `Processor`.
  - Name the actual responsibility: `Watcher`, `Changeset`, `RowBuilder`, `BlobCache`, not `DiffManager`.
  - "Factory" in Rust is `Builder`. To accept "something that makes Foos", take `impl Fn() -> Foo`.
- The same goes for modules: no `utils.rs`/`helpers.rs`/`common.rs`. Put the function where its domain lives.

## Conventions that get forgotten (M-UPSTREAM-GUIDELINES)

- **Conversions (C-CONV):**
  - `as_x()` is a free borrowed view (`as_bytes`).
  - `to_x()` is an expensive or owned copy (`to_string_lossy`).
  - `into_x()` consumes self (`into_rows`).
- **Getters (C-GETTER):** `fn path(&self)`, not `fn get_path(&self)`. Mutable getters are `path_mut()`.
- **Predicates:** `is_x()` / `has_x()` (`is_binary`, `has_changes`).
- **Constructors (C-CTOR):** `new` / `with_x` / `from_x`, as inherent associated fns.
- **Iterators:** `iter()`, `iter_mut()`, `into_iter()`. The iterator types are named after the method (`Iter`, `IntoIter`).
- **Feature names (C-FEATURE):** no placeholder words, so `watch`, not `use-watch` or `with-watch`.
- **Casing:** `UpperCamelCase` types and variants (acronyms as words: `Oid`, `Utf8`, `Tui`), `snake_case` fns and modules, `SCREAMING_SNAKE_CASE` consts.
