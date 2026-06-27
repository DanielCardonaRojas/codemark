# vhs tape: animated demo GIF — the Settings overlay.
#
# Produces: codemark_tui_settings.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Flow:
#   1. open the Settings overlay
#   2. browse the Configuration tab rows
#   3. cycle to the Theme tab and preview a few schemes live
#   4. cycle to the About tab, then step back through the tabs
#
# Key map used (see crates/codemark-tui/src/settings.rs SettingsOverlay::handle_key):
#   ,        toggle the Settings overlay (open / close)
#   ] / Tab  next tab   (Configuration -> Theme -> About)
#   [        previous tab
#   j / k    move the selection within the current tab
#            - Theme tab: live-previews the highlighted scheme

Output "__OUT__/codemark_tui_settings.gif"

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

# --- 1. Open the Settings overlay (starts on the Configuration tab) ----------
Type ","
Sleep 1s

# --- 2. Browse the Configuration rows ----------------------------------------
Down 1
Sleep 500ms
Down 1
Sleep 500ms
Up 1
Sleep 650ms

# --- 3. Configuration -> Theme, then preview a few schemes live --------------
Type "]"
Sleep 800ms
Down 1
Sleep 700ms
Down 1
Sleep 700ms
Down 1
Sleep 800ms

# --- 4. Theme -> About -------------------------------------------------------
Type "]"
Sleep 1s

# --- 5. Step back the other way: About -> Theme -> Configuration -------------
Type "["
Sleep 600ms
Type "["
Sleep 800ms

Sleep 600ms
