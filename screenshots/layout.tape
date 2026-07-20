# vhs tape: animated demo GIF — resizing and cycling pane layouts.
#
# Produces: codemark_tui_layout.gif
#
# The dashboard is not a fixed grid: focus a pane with its number key, then grow
# it with `+` (or shrink with `-`). The preview and details panes cycle through
# their own sizes on each press. This clip walks the preview pane up to full
# width and preview-only, then the details pane, then the left (bookmarks) pane.
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Key map used (see crates/codemark-tui/src/browser/events.rs handle_resize):
#   3        focus the bookmarks (left) pane
#   4        focus the preview pane
#   5        focus the details pane
#   + / -    grow / shrink the focused pane
#            - preview pane: Regular -> Full -> PreviewOnly -> Regular
#            - details pane: Regular -> Half -> Full -> Regular
#            - left pane:    Regular -> Half -> Full -> Regular

Output "__OUT__/codemark_tui_layout.gif"

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

Sleep 500ms

# --- 1. Expand the preview pane (Regular -> Full -> PreviewOnly -> Regular) ---
# `4` focuses the preview; each `+` cycles its layout one step.
Type "4"
Sleep 500ms
Type "+"
Sleep 1.2s
Type "+"
Sleep 1.2s
Type "+"
Sleep 900ms

# --- 2. Expand the details pane (Regular -> Half -> Full -> Regular) ----------
# `5` focuses the details pane; `+` grows it over the right side, then full screen.
Type "5"
Sleep 500ms
Type "+"
Sleep 1.2s
Type "+"
Sleep 1.2s
Type "+"
Sleep 900ms

# --- 3. Expand the bookmarks (left) pane (Regular -> Half -> Full -> Regular) -
# `3` focuses the bookmarks pane; `+` widens the left side until it fills the window.
Type "3"
Sleep 500ms
Type "+"
Sleep 1.2s
Type "+"
Sleep 1.2s
Type "+"
Sleep 900ms

Sleep 600ms
