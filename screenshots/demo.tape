# vhs tape: animated demo GIF — a guided tour of the TUI.
#
# Produces: codemark_tui_demo.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute images dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Flow:
#   1. browse a few bookmarks with the arrow keys (live preview updates)
#   2. focus the preview pane and cycle its Content -> Info -> Query tabs
#   3. switch the left pane to Collections, open one
#   4. step through the collection's bookmarks
#
# Key map used (see crates/codemark-tui/src/browser):
#   Down/Up  move the bookmark selection (Panel3)
#   5        focus the right preview pane (Steps/Content)
#   4        focus the bookmarks pane (Panel3)
#   ] / [    cycle the *focused* pane's tabs
#            - right pane: Content -> Info -> Query
#            - Panel3:     Bookmarks -> Collections -> Tours
#   Enter    open the selected collection (loads it, focuses the right pane)
#   Right    advance to the next step (bookmark) within an open collection

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

Sleep 400ms

# --- 1. Browse bookmarks (preview updates per selection) ---------------------
Down 1
Sleep 500ms
Down 1
Sleep 500ms
Down 1
Sleep 650ms

# --- 1b. Show the contextual keybinding cheat sheet for the bookmarks pane ---
# `?` opens the cheat sheet scoped to the focused pane (here, Panel3/bookmarks);
# press it again to close.
Type "?"
Sleep 1.5s
Type "?"
Sleep 400ms

# --- 2. Inspect the preview tabs: Content -> Info -> Query -------------------
# Focus the right preview pane.
Type "4"
Sleep 450ms

# --- 2b. Show the cheat sheet again — now scoped to the preview pane ---------
Type "?"
Sleep 1.5s
Type "?"
Sleep 400ms
# Content -> Info
Type "]"
Sleep 900ms
# Info -> Query
Type "]"
Sleep 1s
# Back to Content (Query -> Info -> Content).
Type "["
Sleep 300ms
Type "["
Sleep 450ms

# --- 3. Switch to Collections and open one ----------------------------------
# Focus the bookmarks pane, then cycle its tab to Collections.
Type "3"
Sleep 350ms
Type "]"
Sleep 650ms
# Open the selected collection (loads it and focuses the right pane).
Enter
Sleep 1s

# --- 4. Step through the collection's bookmarks -----------------------------
Right 1
Sleep 800ms
Right 1
Sleep 800ms
Right 1
Sleep 900ms

Sleep 600ms
