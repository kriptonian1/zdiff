# Shared by every tape: terminal look, plus a fresh demo repository with zdiff on PATH.
Set Shell "bash"
Set FontSize 16
Set Width 1400
Set Height 800
Set Padding 16
Set TypingSpeed 60ms
Set Theme "GitHub Dark"

Hide
Type `export PATH="$PWD/target/release:$PATH" XDG_CONFIG_HOME="$(mktemp -d)" PS1='$ '` Enter
Type `docs/demo/setup.sh "${TMPDIR:-/tmp}/zdiff-demo" && cd "${TMPDIR:-/tmp}/zdiff-demo" && clear` Enter
Sleep 1s
Show
