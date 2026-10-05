# zdiff

A terminal git diff viewer for watching your changes live. It shows side-by-side or stacked diffs, has a file sidebar, and folds unchanged code. You can also stage and commit from it.

Written in Rust. It reads git in-process with [gitoxide](https://github.com/GitoxideLabs/gitoxide), so it never shells out to `git` to show a diff.

[![CI](https://github.com/kriptonian1/zdiff/actions/workflows/ci.yml/badge.svg)](https://github.com/kriptonian1/zdiff/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/kriptonian1/zdiff)](https://github.com/kriptonian1/zdiff/releases)
![License: MIT](https://img.shields.io/badge/license-MIT-blue)

![zdiff browsing changes: files, folds, split and unified views, all files, go to file](docs/demo/overview.gif)

## Features

- Split, unified, and auto view. Auto follows the pane width.
- A sidebar with a folder tree, a status letter, and `+/-` counts for each file.
- Folds for unchanged code. Open one fold, or all of them, or show more context lines.
- Word-level highlights inside changed lines.
- Syntax highlighting with tree-sitter and GitHub's colors.
- `--watch` refreshes when files change. Only the touched paths get a new diff.
- An All files view stacks every diff in one scroll.
- Find in the current file. Search across all changed files. Fuzzy go-to-file and go-to-line.
- Check files in the sidebar, stage them, and commit without leaving the viewer.
- History, branches, and stashes, each in its own popup.
- Image diffs (PNG, JPEG, GIF, WebP, BMP, ICO, SVG) as pictures, with 2-up, swipe, and onion skin compare.
- Compares branches: a branch against your working tree, or two branches the way a GitHub PR shows them.
- Opens a patch file, or a patch on stdin.
- A menu bar (`F10`), mouse support, 40+ themes, and keys you can rebind.

## Install

### Homebrew (macOS and Linux)

```bash
brew tap kriptonian1/zdiff https://github.com/kriptonian1/zdiff
brew install zdiff
```

### Cargo

```bash
cargo install --git https://github.com/kriptonian1/zdiff zdiff
```

### Prebuilt binary

Download `zdiff-<target>.tar.gz` from [Releases](https://github.com/kriptonian1/zdiff/releases). Builds exist for macOS (Apple Silicon, Intel) and Linux (arm64, x86_64).

## Quick start

Run it inside a git repository:

```bash
zdiff                        # your changes against HEAD
zdiff --watch                # refresh as you edit
zdiff -c main                # main against your working tree
zdiff -c main feature        # what feature changed since it split from main
zdiff -f src 'tests/**/*.rs' # only these paths or globs
zdiff --read-only            # no staging or committing
zdiff --patch fix.patch      # view a patch file
git diff main | zdiff --patch -   # view a patch from stdin
```

Basic keys:

| Key | Does |
|---|---|
| `j` / `k` | Scroll the diff, or move in the sidebar |
| `n` / `p` | Next / previous file |
| `]` / `[` | Next / previous change |
| `Tab` | Switch focus between sidebar and diff |
| `v` | Switch split / unified |
| `a` | Switch single file / all files |
| `Enter` | Open a fold |
| `Ctrl+P` | Go to file |
| `Ctrl+F` | Find in file |
| `L` | History |
| `F10` | Menu |
| `Ctrl+K` | All shortcuts |
| `q` | Quit |

Read the [guide](docs/guide.md) for all features, keys, and settings.

## Demos

**Watch mode.** Edits made in another program show up on their own.

![zdiff --watch picking up edits](docs/demo/watch.gif)

**Stage and commit.** Check files, stage them, and write the message.

![Staging files and writing a commit message](docs/demo/stage.gif)

**Branches, history, and stashes.**

![The branches, history, and stash popups](docs/demo/history.gif)

## Comparison

| | zdiff | [hunk](https://github.com/modem-dev/hunk) | [lumen](https://github.com/jnsahaj/lumen) | [delta](https://github.com/dandavison/delta) | [difftastic](https://github.com/Wilfred/difftastic) |
|---|:-:|:-:|:-:|:-:|:-:|
| Interactive review UI | ✅ | ✅ | ✅ | ❌ | ❌ |
| File sidebar | ✅ | ✅ | ✅ | ❌ | ❌ |
| Split and unified views | ✅ | ✅ | ✅ | ✅ | ✅ |
| Auto layout by width | ✅ | ✅ | ❌ | ❌ | ❌ |
| Syntax highlighting | ✅ | ✅ | ✅ | ✅ | ✅ |
| Live refresh while you edit | ✅ | ✅ | ✅ | ❌ | ❌ |
| Mouse support | ✅ | ✅ | ✅ | ❌ | ❌ |
| Stage and commit | ✅ | ❌ | ❌ | ❌ | ❌ |
| History, branches, stashes | ✅ | ❌ | ❌ | ❌ | ❌ |
| Image diffs as pictures | ✅ | ❌ | ❌ | ❌ | ❌ |
| AI or agent features | ❌ | ✅ | ✅ | ❌ | ❌ |
| Structural (AST) diff | ❌ | ❌ | ❌ | ❌ | ✅ |
| Works as a `git` pager | ❌ | ✅ | ❌ | ✅ | ✅ |

Pick by job:

- **zdiff**: keep it open beside your editor, watch changes, then stage and commit.
- **hunk**: review a change set with AI or agent notes beside the code.
- **lumen**: review diffs and PRs, and use AI for commit messages and explanations.
- **delta**: make `git diff` and `git log -p` output look better, with no new tool to learn.
- **difftastic**: diff by syntax tree, so formatting changes don't show as edits.
- **lazygit / gitui**: a full git client. zdiff is a diff viewer first, with a small set of git actions.

This table comes from each project's README as of October 2026. If a cell is wrong, open an issue.

## Build from source

The toolchain is pinned in `rust-toolchain.toml`.

```bash
git clone https://github.com/kriptonian1/zdiff
cd zdiff
cargo build --release        # target/release/zdiff
cargo test --workspace
```

The demos are [vhs](https://github.com/charmbracelet/vhs) tapes in `docs/demo`. Each tape builds a throwaway repository with `setup.sh`, which uses macOS `sed`. To record one again, build a release binary and run this from the repository root:

```bash
vhs docs/demo/overview.tape
```

## License

MIT
