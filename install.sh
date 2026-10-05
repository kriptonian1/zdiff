#!/bin/sh
# Installs zdiff with Homebrew, or from the latest release when brew is missing or fails.
#   curl -fsSL https://raw.githubusercontent.com/kriptonian1/zdiff/main/install.sh | sh
set -eu

repo=kriptonian1/zdiff

from_release() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    *)
      echo "zdiff: no prebuilt binary for $(uname -s) $(uname -m). Use cargo install instead." >&2
      exit 1
      ;;
  esac
  bin="$HOME/.local/bin"
  mkdir -p "$bin"
  curl -fsSL "https://github.com/$repo/releases/latest/download/zdiff-$target.tar.gz" |
    tar -xz -C "$bin" zdiff
}

# If brew fails for any reason, the release binary still gets the user going.
if command -v brew >/dev/null 2>&1 &&
  brew tap "$repo" "https://github.com/$repo" &&
  brew install --cask "$repo/zdiff"; then
  bin="$(brew --prefix)/bin"
else
  command -v brew >/dev/null 2>&1 && echo "brew failed; installing the release binary instead."
  from_release
fi

# macOS and most Linux systems ship an unrelated /usr/bin/zdiff; ours has to win.
if [ "$(command -v zdiff || true)" = "$bin/zdiff" ]; then
  echo "Installed $("$bin/zdiff" --version). Run zdiff inside a git repository."
  exit 0
fi

case "${SHELL##*/}" in
  zsh) rc="$HOME/.zshrc" ;;
  bash) rc="$HOME/.bashrc" ;;
  *) rc="$HOME/.profile" ;;
esac
line="export PATH=\"$bin:\$PATH\""
grep -qxF "$line" "$rc" 2>/dev/null || printf '\n%s\n' "$line" >>"$rc"
echo "Installed $("$bin/zdiff" --version), and put $bin first in PATH in $rc."
echo "Open a new terminal, then run zdiff inside a git repository."
