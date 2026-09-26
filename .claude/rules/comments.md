---
paths:
  - "**/*.{rs,toml}"
---

# Comments

Default: do not write comments. Well-named symbols and small functions
are the documentation. Only comment when a rule below explicitly allows it.

## Never write
- Comments restating what the code does (`// increment counter`)
- Section banners, closing-brace markers, ASCII dividers
- Attribution, changelog, or "added by" notes — git has this
- Commented-out code — delete it
- Narration of your own reasoning or of this chat session
- JSDoc/TSDoc that only repeats the type signature

## Write only when
- WHY is non-obvious: a workaround, a perf trade-off, a spec quirk.
  Include the cause: link an issue, RFC, or ticket.
- Warning of consequence: "not thread-safe", "O(n²), keep n < 100"
- `// TODO(name): <what>` or `// FIXME(name): <what>` with a ticket ref
- Public API of a shared package: one line on contract, units, ownership
  of returned data — not on how it's implemented
- Legal/license headers required by the repo

## Style
- One line. Two only if the second carries information the first cannot.
- Max ~80 characters per line. Never a paragraph.
- Drop the preamble: "This is because the API returns stale data" →
  "API returns stale data here"
- No hedging ("might", "possibly", "for some reason"). If you don't know
  why, say so in one clause and link the issue.
- Sentence fragments are fine. Skip articles and filler words.
- If it takes more than two lines to explain, the code is wrong —
  refactor instead, or move the explanation to a doc and link it.

## Rules
- Comment sits directly above the code it describes, never trailing far away
- If you need a comment to explain a block, extract a named function instead
- When you change code, update or delete its comment in the same edit
- English only
