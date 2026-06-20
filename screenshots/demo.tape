# vhs tape: animated demo GIF (browsing bookmarks).
#
# Produces: codemark_tui_demo.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute images dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Walks down through several bookmarks (and back up) with the arrow keys so the
# live content preview updates per selection — showing off the browse loop. The
# launch is hidden so the GIF opens straight on the loaded interface, and the
# framerate is kept modest to keep the file README-friendly.

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

Sleep 800ms

# Move down through bookmarks; pause on each so the preview pane re-renders.
Down 1
Sleep 1.1s
Down 1
Sleep 1.1s
Down 1
Sleep 1.1s
Down 1
Sleep 1.1s
Down 1
Sleep 1.1s

# Back up to the top.
Up 1
Sleep 800ms
Up 1
Sleep 800ms
Up 1
Sleep 800ms
Up 1
Sleep 800ms
Up 1
Sleep 1.3s
