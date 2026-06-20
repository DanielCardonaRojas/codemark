# vhs tape: main browser view (bookmarks list + content preview)
#
# Produces: codemark_tui_screenshot.png
#
# Run via screenshots/capture.sh, which sets the working directory to the seeded
# demo repo and points PATH at the built binaries. The TUI auto-detects the
# demo's .codemark/codemark.db from the current directory.
#
# On a seeded database the TUI opens focused on the Bookmarks tab (Panel3) with
# the Content preview already showing the first bookmark — i.e. the "main" view
# needs no navigation. We just select the first bookmark and let it render.


Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"

# Launch the TUI from the seeded demo repo.
Type "codemark-tui"
Enter
Sleep 2.5s

# Preview the first bookmark so the content pane is populated.
Enter
Sleep 1.5s

Screenshot "__OUT__/codemark_tui_screenshot.png"
