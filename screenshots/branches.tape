# vhs tape: animated demo GIF — filter collections by branch, then clone one
# onto another branch.
#
# Produces: codemark_tui_branches.gif
#
# The fixture (see seed-fixture.sh) has collections on `main` plus a
# `sync-retry` feature branch holding the `tour-sync` collection. This clip
# filters the Collections list by each branch, then copies `server-auth` from
# `main` onto `sync-retry` with the branch picker, and finishes on the
# `sync-retry` filter showing the new copy next to `tour-sync`.
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the seeded fixture clone so the TUI uses the fixture DB (see
# seed-fixture.sh), not the repo's live .codemark/codemark.db.
#
# Key map used (see crates/codemark-tui/src/browser):
#   2 / 3    focus the Filters pane / the Collections pane
#   ]        switch the Filters pane from Tags to Branches
#   j / k    move down / up a row
#   Space    toggle the selected branch filter (rows sort by name: main, sync-retry)
#   c        copy the selected collection to a branch (opens the branch picker)
#   <typing> narrow the picker; Enter picks the highlighted branch
#   Right    move the confirmation dialog from Cancel (default) to Confirm
#
# After the copy the TUI rebuilds the Branches list with no filter active and
# the cursor back on the first row (main), which is why the last step is `j`.

Output "__OUT__/codemark_tui_branches.gif"

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

# --- 1. Filter the Collections list by branch --------------------------------
Type "2"
Sleep 400ms
Type "]"
Sleep 800ms
# sync-retry: only the feature branch's collection is left.
Type "j"
Sleep 400ms
Space
Sleep 1.8s
# Swap to main: the main collections come back, tour-sync drops out.
Space
Sleep 300ms
Type "k"
Sleep 400ms
Space
Sleep 1.8s

# --- 2. Clone server-auth onto the feature branch ----------------------------
Type "3"
Sleep 500ms
Type "jj"
Sleep 900ms
Type "c"
Sleep 1.2s
Type@150ms "sync"
Sleep 900ms
Enter
# The copy is planned against the branch tip; the dialog reports matches.
Sleep 2.2s
Right
Sleep 600ms
Enter
# The copy opens on its first step.
Sleep 2.5s

# --- 3. The copy now lives on sync-retry --------------------------------------
Type "2"
Sleep 400ms
Type "j"
Sleep 400ms
Space
Sleep 3s
