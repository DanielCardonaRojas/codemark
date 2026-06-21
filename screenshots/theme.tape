# vhs tape: per-theme gallery shot (main browser view).
#
# Produces: codemark_tui_theme_<slug>.png (one per base16 scheme).
#
# Rendered once per scheme by capture.sh, which substitutes:
#   __OUT__   -> repo root (absolute output dir)
#   __SHOT__  -> the per-theme output filename
# and exports CODEMARK_TUI_THEME so the TUI renders in that scheme.
#
# Padding is 0 so the TUI fills the frame edge-to-edge: no terminal background
# is visible, which means vhs's own `Set Theme` is irrelevant and no
# TUI-theme -> vhs-theme name mapping is needed. Only base16/base24 schemes
# (see `codemark-tui --list-schemes`) re-theme the whole chrome, so the gallery
# is limited to those.

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 0

Type "codemark-tui"
Enter
Sleep 2.5s

Screenshot "__OUT__/__SHOT__"
