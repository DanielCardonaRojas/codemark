# vhs tape: collections view (Panel3's Collections tab)
#
# Produces: codemark_tui_collections_screenshot.png
#
# Panel3 has Bookmarks / Collections / Tours tabs. On startup Panel3 is focused
# on Bookmarks, so advancing its tabs once with ] reaches Collections.
#
# NOTE: confirm the ] count on the first local run; adjust if needed.


Set FontSize 16
Set Width 1400
Set Height 850
Set Padding 20
Set Theme "Catppuccin Mocha"

Type "codemark-tui"
Enter
Sleep 2.5s

# Panel3 is focused on startup; advance its tab to Collections.
Type "]"
Sleep 1s

# Open the collection to show its steps/details.
Enter
Sleep 1.5s

Screenshot "__OUT__/codemark_tui_collections_screenshot.png"
