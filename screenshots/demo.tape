# vhs tape: animated demo GIF — a guided tour of the TUI.
#
# Produces: codemark_tui_demo.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute images dir
# and runs from the seeded fixture clone so the TUI uses the fixture DB (see
# seed-fixture.sh), not the repo's live .codemark/codemark.db.
#
# Flow:
#   1. browse collections (the preview shows each one's overview), then return
#      to the top one: bookmark-resolution, which seed-fixture.sh creates last
#      so it lists first
#   2. open it and step through its bookmarks; the 4th step (relax_query) is
#      Drifted, because seed-fixture.sh edits it after bookmarking
#   3. on a step, cycle the preview tabs: Steps (code) -> Info -> Query
#   4. go back and switch the pane to Bookmarks; the newest bookmark
#      (build_auth_headers) is Broken, its function removed by the seed
#
# Key map used (see crates/codemark-tui/src/browser):
#   Down/Up  move the selection in the focused list
#   ?        contextual keybinding cheat sheet (press again to close)
#   Enter    open the selected collection (focuses the right pane, step 1)
#   Right    next step within an open collection
#   ] / [    cycle the *focused* pane's tabs
#            - right pane (open collection): Steps -> Info -> Query
#            - pane 3:                       Collections -> Bookmarks
#   Esc      back from an open collection
#   3        focus pane 3 (Collections / Bookmarks)

Output "__OUT__/codemark_tui_demo.gif"

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"
Set Framerate 12

# Launch off-camera so the GIF starts on the loaded TUI, not the typing.
Hide
Type "codemark-tui"
Enter
Sleep 2.5s
Show

Sleep 600ms

# --- 1. Browse collections (preview shows each overview), back to the top ----
Down 1
Sleep 700ms
Down 1
Sleep 700ms
Up 2
Sleep 900ms

# Contextual cheat sheet for the collections pane.
Type "?"
Sleep 1.5s
Type "?"
Sleep 400ms

# --- 2. Open bookmark-resolution and step through it ------------------------
Enter
Sleep 1.3s
Right 1
Sleep 1s
Right 1
Sleep 1s
# Step 4: relax_query, Drifted (its body changed after it was bookmarked).
Right 1
Sleep 1.3s

# --- 3. Preview tabs on this step: Steps -> Info -> Query -------------------
Type "]"
Sleep 1.5s
Type "]"
Sleep 1.3s
Type "["
Sleep 300ms
Type "["
Sleep 600ms

# --- 4. Back, then the Bookmarks tab (newest first; top one is Broken) ------
Escape
Sleep 500ms
Type "3"
Sleep 300ms
Type "]"
Sleep 1.3s
Down 1
Sleep 900ms
Down 1
Sleep 900ms

Sleep 600ms
