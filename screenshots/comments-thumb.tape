# vhs tape: static thumbnail — comment threads beside the code.
#
# Produces: codemark_tui_comments_thumb.png
#
# A single still used as the poster/thumbnail for the comments demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the seeded
# fixture clone so the TUI uses the fixture DB (see seed-fixture.sh). The tape
# writes comments into the fixture DB; capture.sh restores the DB afterwards.
#
# Flow: off-camera, an agent comments on run_query and a reviewer replies (both
# via the CLI); open the tour at full width, step to run_query, `+` to the
# side-by-side layout, capture.

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"

Hide
Type "BM=$(codemark list --file crates/codemark-core/src/query/matcher.rs --format line --line-format '{ID}')"
Enter
Type `C=$(codemark comment add $BM --range 24-25 --created-by claude --body "This compiles the query on every call. Cache compiled queries per language?" | sed -n 's/^ *"id": "\(.*\)",$/\1/p')`
Enter
Type `codemark comment reply $C --created-by Ana --body "Agreed: a per-language cache keyed by the query string." >/dev/null`
Enter
Type `codemark comment add $BM --range 31 --created-by Ana --body "Reuse one QueryCursor across calls? Resetting it is cheap." >/dev/null`
Enter
Type "codemark-tui --collection bookmark-resolution --layout preview"
Enter
Sleep 2.5s
Right
Sleep 1.5s
Type "+"
Sleep 1s
Type "n"
Show
Sleep 800ms

Screenshot "__OUT__/codemark_tui_comments_thumb.png"
