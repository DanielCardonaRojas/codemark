# vhs tape: animated demo GIF — filtering pane contents.
#
# Produces: codemark_tui_filter.gif
#
# Filtering is distinct from search: `/` narrows the *visible list* of the
# focused pane (live, as you type), whereas search queries the database. This
# clip shows filtering the Bookmarks list, then the Tags pane, to highlight that
# filters are pane-scoped.
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the seeded fixture clone so the TUI uses the fixture DB (see
# seed-fixture.sh), not the repo's live .codemark/codemark.db.
#
# Key map used (see crates/codemark-tui/src/main.rs and src/browser):
#   ]        switch pane 3 (focused at startup) from Collections to Bookmarks
#   /        enter filter mode for the focused pane
#   <typing> narrows that pane's list live (no Enter needed)
#   Esc      clear the filter
#   2        focus the Tags pane

Output "__OUT__/codemark_tui_filter.gif"

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

# --- 1. Filter the Bookmarks list ---------------------------------------------
# Pane 3 starts on Collections; `]` switches it to Bookmarks. `/` enters filter
# mode; typing narrows the list live.
Type "]"
Sleep 700ms
Type "/"
Sleep 500ms
Type "auth"
Sleep 1.8s
# Clear the filter.
Escape
Sleep 800ms

# --- 2. Filter a different pane — focus the Tags pane, then filter it --------
# `2` focuses the Tags pane. Filtering it is independent of the Bookmarks
# filter above — filters are pane-scoped.
Type "2"
Sleep 400ms
Type "/"
Sleep 500ms
Type "concept"
Sleep 1.8s
Escape
Sleep 800ms

Sleep 600ms
