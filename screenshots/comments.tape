# vhs tape: animated demo GIF — anchored code comments, from an agent and a person.
#
# Produces: codemark_tui_comments.gif
#
# Flow:
#   1. An agent leaves a comment programmatically: `codemark comment add` with
#      --created-by claude and a line range inside the bookmark. The JSON reply
#      shows it anchored to the code.
#   2. The TUI opens on the bookmark-resolution collection (preview at full
#      width) and steps to `run_query`, where the agent's comment sits inline
#      under the lines it points at.
#   3. Reply to it (n jumps to the comment, r replies), add a new comment on
#      another line (c), and edit it (E). Drafts save with Ctrl+S.
#   4. `+` switches to the side-by-side layout: code on the left, its comment
#      threads beside it. n/N walk the threads; d deletes one (after confirming).
#   5. `-` returns to the inline layout.
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the seeded fixture clone so the TUI uses the fixture DB (see
# seed-fixture.sh), not the repo's live .codemark/codemark.db. The tape writes
# comments into the fixture DB; capture.sh restores the DB afterwards.
#
# Key map used (see crates/codemark-tui/src/browser/comment_draft.rs):
#   →        next step              n / N    next / previous comment
#   r        reply to the comment   c        comment on the line under the cursor
#   E        edit your comment      d        delete the thread (Tab, Enter confirms)
#   Ctrl+S   save the draft         + / -    cycle the preview layout

Output "__OUT__/codemark_tui_comments.gif"

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"
Set Framerate 12
Set TypingSpeed 35ms

# Off-camera: a stable reviewer name for the TUI-authored comments (the TUI
# signs comments with git's user.name) and the id of the bookmark the agent
# comments on (ids are random per seed).
Hide
Type "git config user.name Ana; BM=$(codemark list --file crates/codemark-core/src/query/matcher.rs --format line --line-format '{ID}'); clear"
Enter
Sleep 800ms
Show

# --- 1. The agent comments on lines 24-25 of run_query -----------------------
Type `codemark comment add $BM --range 24-25 --created-by claude --body "This compiles the query on every call. Cache compiled queries per language?"`
Sleep 400ms
Enter
Sleep 2.5s

# --- 2. Open the tour and step to run_query ----------------------------------
Type "codemark-tui --collection bookmark-resolution --layout preview"
Sleep 300ms
Enter
Sleep 2.5s
Right
Sleep 1.8s

# --- 3. Reply, comment, edit (inline layout) ----------------------------------
# n puts the cursor on the agent's comment; r opens a reply draft.
Type "n"
Sleep 800ms
Type "r"
Sleep 600ms
Type "Agreed: a per-language cache keyed by the query string."
Sleep 600ms
Ctrl+S
Sleep 1.5s

# Move down to `let mut cursor = QueryCursor::new();` and comment on it.
Down 6
Sleep 700ms
Type "c"
Sleep 600ms
Type "Reuse one QueryCursor across calls?"
Sleep 600ms
Ctrl+S
Sleep 1.5s

# E reopens your latest comment in the thread under the cursor.
Type "E"
Sleep 700ms
Type " Resetting it is cheap."
Sleep 600ms
Ctrl+S
Sleep 1.8s

# --- 4. Side-by-side layout: threads beside the code --------------------------
Type "+"
Sleep 2s
Type "N"
Sleep 1.5s
Type "n"
Sleep 1.5s

# Delete the QueryCursor thread: d asks first; Tab moves to Confirm.
Type "d"
Sleep 1.2s
Tab
Sleep 500ms
Enter
Sleep 1.8s

# --- 5. Back to the inline layout ----------------------------------------------
Type "-"
Sleep 2s
