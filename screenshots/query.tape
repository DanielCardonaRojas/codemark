# vhs tape: query preview view (the tree-sitter Query tab of the right pane)
#
# Produces: codemark_tui_query_screenshot.png
#
# The right pane has Content / Info / Query tabs. Tabs cycle with [ and ] for
# the *focused* pane, so we first move focus to the right pane (Tab cycles
# focus across areas) and then advance to the Query tab.
#
# NOTE: the exact number of Tab presses to land focus on the right pane, and of
# ] presses to reach the Query tab, are best confirmed on the first local run —
# adjust the counts below if the captured screen isn't the Query tab.


Set FontSize 16
Set Width 1400
Set Height 850
Set Padding 20
Set Theme "Catppuccin Mocha"

Type "codemark-tui"
Enter
Sleep 2.5s

# Select the first bookmark so the right pane has content to show.
Enter
Sleep 1s

# Move focus to the right pane (content/preview area).
Tab
Sleep 500ms

# Advance the right-pane tabs to Query (Content -> Info -> Query).
Type "]"
Sleep 300ms
Type "]"
Sleep 1.5s

Screenshot ${OUT_DIR}/codemark_tui_query_screenshot.png
