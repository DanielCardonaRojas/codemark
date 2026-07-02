# Design: Peer-to-Peer Sharing in the TUI

> Status: approved, in implementation. Builds on the CLI p2p feature
> (`codemark-p2p`, `proposals/000–004`). All new behavior is behind the TUI's
> optional `p2p` feature; a default build is unchanged.

## Goal
Let a user share and receive tours over p2p from inside the TUI, reusing the
`codemark-p2p` transport and the `build_pack_bytes` / `import_pack_bytes` core
helpers. Coexists with — does not replace — the existing Codetours server flows.

## Keybindings (with `--features p2p`)
| Tab | Key | Action |
| :-- | :-- | :-- |
| Collections | `P` | Push method dialog: `[ Codetours server ] [ Peer-to-peer ]` |
| Collections | `p` | **New:** paste a p2p ticket → import as a new local collection |
| Collections | `Ctrl+E` | Serving menu (only visible while serving) |
| Tours | `p` / `P` | Unchanged (server pull/push) |

Rationale: push acts on a local collection (Collections tab); server pull acts on
a selected remote tour (Tours tab). They already live in different tabs, so p2p
pull gets its own home on the currently-unbound Collections-`p`, and no "choose
pull method" menu is needed. With the feature **off**, `P`/`p` behave exactly as
today (no dialogs, no bottom-bar changes).

## Push flow (Collections tab, `P`)
1. If p2p on **and** a server is configured → method dialog. If p2p on and no
   server configured → straight to Peer-to-peer. If p2p off → today's server push.
2. Peer-to-peer chosen:
   a. Build the pack bytes in a `spawn_blocking` task (reopen DB by path, exactly
      like `start_push_collection`; `Database` is not `Send`).
   b. Move the bytes to a normal async task → `codemark_p2p::push_bytes` →
      `(ticket, Provider)`. No DB in this task, so `Send` is satisfied.
   c. Copy the ticket to the clipboard; post `Event::P2pServing { name, ticket }`.
3. UI: toast "Ticket copied — serving '<name>'" and a bottom-bar `● serving
   '<name>'` indicator.

### Serving lifecycle
- One active serving at a time; starting another push asks to replace it.
- Runs until the user stops it or the TUI quits. The `Provider` and a `oneshot`
  stop-sender live in `BrowserLayout`; stopping (or the layout dropping on quit)
  drops the `Provider`, shutting down the iroh node.
- Stop affordance: while serving, the bottom bar shows `Ctrl+E` (currently free)
  → a small **serving menu**: `[ Re-copy ticket ] [ Stop serving ] [ Close ]`.
  The menu also shows the raw ticket text, so a failed clipboard copy is
  recoverable.
- Stretch (verify during impl): iroh-blobs exposes a provider event channel
  (`BlobsProtocol::new(store, Some(events))`); if wiring is clean, auto-stop on
  first successful delivery with a "Delivered to peer ✓" toast. Otherwise ship
  manual-stop only.

## Pull flow (Collections tab, `p`)
1. p2p off → no-op (as today). p2p on → open the **paste-ticket modal**.
2. Modal: a single-line text input (reuse the existing filter/search input
   widget), pre-filled from the clipboard iff it looks like a `blob…` ticket,
   editable. Enter → pull; Esc → cancel.
3. `spawn_blocking` + `block_on`: `codemark_p2p::pull_bytes` →
   `import_pack_bytes` (reopen DB by path) → returns the existing `ImportedTour`.
4. Success → toast "Imported '<name>' (<n> bookmarks)" + refresh Collections;
   failure → toast with the transport error (already flags truncation).

## Events
New `Event` variants, handled in `handle_app_event` like `SyncComplete`:
- `P2pServing { name, ticket }` — serving started, update indicator + toast.
- `P2pServingStopped` — clear indicator.
- `P2pPullComplete(Result<ImportedTour, String>)` — toast + refresh.
- `P2pDelivered` — stretch; auto-stop + toast.

## Error handling
Build / bind / clipboard / pull errors surface as toasts. Serving never starts if
the pack build or `push_bytes` fails. The serving menu's raw-ticket display is the
clipboard-independent fallback.

## Testing
- Transport + pack round-trip already covered (`codemark-p2p` tests,
  `scripts/test_p2p_local.sh`, core).
- New pure-logic unit test for the push-method availability decision
  (feature flag + server-configured → which flow).
- TUI rendering/interaction verified manually (no TUI render-test harness today).

## Out of scope (now)
p2p in the Tours tab; manual ticket entry as a file; multi-peer concurrent
serving; live/collaborative tours (`iroh-docs`).
