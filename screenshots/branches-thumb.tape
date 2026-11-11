# vhs tape: static thumbnail — branch filtering and cloning a collection.
#
# Produces: codemark_tui_branches_thumb.png
#
# A single still used as the poster/thumbnail for the branches demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the
# seeded fixture clone so the TUI uses the fixture DB (see seed-fixture.sh).
#
# Flow: filter the Collections list to main (2, ], Space), select server-auth
# (3, jj), open the copy-to-branch picker (c), type a branch, capture.

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"

Hide
Type "codemark-tui"
Enter
Sleep 2.5s
Show

# Filter to main so the Branches tab shows a checked branch behind the picker.
Type "2"
Sleep 400ms
Type "]"
Sleep 500ms
Space
Sleep 600ms
Type "3"
Sleep 400ms
Type "jj"
Sleep 600ms
Type "c"
Sleep 1s
Type "sync"
Sleep 1s

Screenshot "__OUT__/codemark_tui_branches_thumb.png"
