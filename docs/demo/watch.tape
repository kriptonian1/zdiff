# --watch: edits made outside zdiff show up on their own.
Source docs/demo/common.tape
Output docs/demo/watch.gif

Hide
Type `(sleep 4; sed -i '' 's/Some(PathBuf::from(home)/Some(PathBuf::from(\&home)/' src/config.rs; sleep 3; printf '\n## Install\n\n    cargo install forecast\n' >> README.md) >/dev/null 2>&1 & disown; clear` Enter
Sleep 300ms
Show
Type "zdiff --watch" Sleep 300ms Enter
Sleep 2s
Down Sleep 1s
Down Sleep 1s
Down Sleep 4s
Down Sleep 3.5s
Type "q"
Sleep 500ms
