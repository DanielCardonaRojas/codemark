# vhs tape: query preview view (the tree-sitter Query tab of the right pane)
#
# Produces: codemark_tui_query_screenshot.png
#
# The right pane has Content / Info / Query tabs. `]`/`[` cycle the tabs of the
# *focused* pane, and the right pane is focused with the `5` number key (Tab
# only cycles the left-side panes). So: focus the right pane, then advance two
# tabs (Content -> Info -> Query).

Set FontFamily "FiraCode Nerd Font Mono"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"

Type "codemark-tui"
Enter
Sleep 2.5s

# On a seeded DB the first bookmark is already selected, so the Content tab is
# populated. Focus the right pane (5 = right/steps pane).
Type "5"
Sleep 500ms

# Advance the right-pane tabs: Content -> Info -> Query.
Type "]"
Sleep 300ms
Type "]"
Sleep 1.5s

Screenshot "__OUT__/codemark_tui_query_screenshot.png"
