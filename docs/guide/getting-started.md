# Quickstart

::: tip The recommended way to use Codemark
You'll rarely type `codemark` yourself. Install the **skill** into your coding
agent, and it will create, recall, and annotate bookmarks for you — across
sessions, even after the code has moved.
:::

Codemark stops you from struggling to find the code you and your agent have
**already discovered**. Instead of re-exploring the codebase every session, your
agent remembers where the important code lives and why — and can leave targeted
comments for the next agent (or you) to pick up.

## 1. Install Codemark

Prebuilt binaries for **macOS**, **Linux**, and **Windows**:

::: code-group

```bash [Homebrew]
brew install DanielCardonaRojas/codemark/codemark
```

```bash [macOS / Linux]
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/DanielCardonaRojas/codemark/releases/latest/download/codemark-cli-installer.sh | sh
```

```powershell [Windows]
powershell -ExecutionPolicy Bypass -c "irm https://github.com/DanielCardonaRojas/codemark/releases/latest/download/codemark-cli-installer.ps1 | iex"
```

```bash [mise]
mise use -g github:DanielCardonaRojas/codemark
```

```bash [Cargo]
cargo install --git https://github.com/DanielCardonaRojas/codemark codemark-cli
# Requires Rust 1.85+; SQLite is bundled.
```

:::

Codemark is **repo-aware by default** — it detects the current Git repository
automatically and stores bookmarks alongside it. No setup required.

## 2. Install the skill into your agent

This is the step that changes everything. One command teaches your agent to
create and recall structural bookmarks on its own:

```bash
codemark install-skill --agent claude --scope user
```

Works with **Claude Code**, **GitHub Copilot**, **Gemini CLI**, and any agent
that loads `.agents/skills`. The context your agent builds in one session is
still there in the next — no more rebuilding from scratch.

## 3. Just ask

That's it. Open a session in any repo and talk to your agent normally — it
handles the bookmarking.

### Remember what you've already found

> *"Trace how a request flows from the HTTP router to the database. Create a
> collection called `request-lifecycle` and bookmark each key hop: the route
> handler, the auth middleware, the service layer, and the query builder. Add a
> short note to each explaining its role."*

Next session — even after the code has moved:

> *"Load the `request-lifecycle` collection and walk me through it."*

### Leave targeted comments for agents

Bookmarks carry two channels. **Notes** are durable explanations of what the code
is; **comments** are markdown discussion tied to a task or ticket — perfect for
leaving targeted guidance for the next agent:

> *"Bookmark the token-refresh logic and add a comment: investigating the race
> in TICKET-42 — see PR #116. The refresh path is the likely culprit."*

The next agent reads that comment and starts exactly where the last one left off.

### Real-world use cases

| Goal | Ask your agent |
|------|----------------|
| 🧭 Onboard | *"Load the `request-lifecycle` collection and give me a guided tour, in the order the code runs."* |
| 🔎 Explain a flow | *"Bookmark the checkout flow into a `checkout` collection, then summarize each step."* |
| 🐞 Hunt a bug | *"There's a bug where expired tokens are still accepted. Read the `auth-flow` collection and tell me which hop is responsible."* |

For the full prompt playbook — discovery strategies, the tag taxonomy, and
notes-vs-comments in depth — see the [Agent Skill](./agent-skills) reference.

---

## Power users: the manual CLI

Prefer to drive it yourself, or scripting? The CLI is fully featured.

```bash
codemark add --file src/auth.rs --range 42 --tag auth \
  --note "token validation entrypoint"
codemark search "auth"          # full-text + semantic search
codemark tour show login-flow --format markdown  # render a collection
codemark tui                    # the keyboard-driven dashboard
```

See the [CLI Reference](./cli-reference) for every subcommand and flag, and
[Keybindings](./keybindings) for the dashboard.
