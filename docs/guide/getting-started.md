# Installation & Quickstart

## Installation

Prebuilt binaries are published for **macOS** (Apple Silicon & Intel), **Linux** (x86_64, glibc), and **Windows** (x86_64). Choose whichever method you prefer.

### Homebrew (macOS / Linux)
```bash
brew install DanielCardonaRojas/codemark/codemark
```

### Install script (macOS / Linux)
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/DanielCardonaRojas/codemark/releases/latest/download/codemark-cli-installer.sh | sh
```

### PowerShell (Windows)
```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/DanielCardonaRojas/codemark/releases/latest/download/codemark-cli-installer.ps1 | iex"
```

### mise
```bash
mise use -g github:DanielCardonaRojas/codemark
```

### Cargo (build from source)
```bash
cargo install --git https://github.com/DanielCardonaRojas/codemark codemark-cli
```
*Requires Rust 1.85+ (edition 2024). SQLite is bundled.*

---

## Quickstart

**Repo-aware by default.** Codemark automatically detects the current Git repository (walking up from your working directory) and stores bookmarks alongside it, with no setup required. 

Prefer to drive it yourself instead of through an agent? The CLI is all you need.

### 1. Bookmark a range
```bash
codemark add --file src/auth.rs --range 42-67 --tag auth --note "token validation entrypoint"
```

### 2. Find it again, even after the code moves
```bash
codemark list                 # see everything you've marked
codemark resolve <id>         # re-locate a single bookmark
codemark search "auth"        # full-text + semantic search
```

### 3. Browse with the dashboard
```bash
codemark tui
```
*(Requires a [Nerd Font](https://www.nerdfonts.com/) for icons).*
