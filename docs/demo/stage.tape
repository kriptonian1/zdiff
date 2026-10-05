# Check files in the sidebar, stage them, and write a commit message.
Source docs/demo/common.tape
Output docs/demo/stage.gif

Type "zdiff" Sleep 300ms Enter
Sleep 1.5s
Space Sleep 700ms
Down Sleep 500ms
Space Sleep 700ms
Type "s" Sleep 1.5s
Type "i" Sleep 500ms
Type@70ms "ci: lint with clippy" Sleep 1.5s
Screenshot docs/demo/stage.png
Escape Sleep 500ms
Type "q"
Sleep 500ms
