#!/usr/bin/env bash
#
# End-to-end smoke test for `tour push/pull --p2p` on a SINGLE machine.
#
# Runs two isolated `codemark` instances (separate --db files) as two OS
# processes: a provider that serves a collection over iroh, and a receiver that
# pulls it back via the printed ticket file. Exercises the whole pipeline —
# pack build, iroh serve, dial, BLAKE3-verified download, import — over the
# machine's own network stack (direct/loopback addresses). It does NOT simulate
# cross-NAT traversal; that requires a real second host (or a Docker NAT lab).
#
# Usage: scripts/test_p2p_local.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

echo "==> Building codemark --features p2p"
cargo build --features p2p -p codemark-cli
BIN="$PWD/target/debug/codemark"

WORK="$(mktemp -d)"
PUSH_PID=""
cleanup() {
  [ -n "$PUSH_PID" ] && kill "$PUSH_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# A throwaway git repo so repo/context detection has something to look at.
REPO="$WORK/repo"
mkdir -p "$REPO"
git -C "$REPO" init -q
git -C "$REPO" config user.email test@example.com
git -C "$REPO" config user.name test
echo "hello" > "$REPO/file.txt"
git -C "$REPO" add -A
git -C "$REPO" commit -qm init

SENDER_DB="$WORK/sender.db"
RECEIVER_DB="$WORK/receiver.db"
PUSH_OUT="$WORK/push.out"
COLLECTION="p2p-smoke"

echo "==> Creating collection '$COLLECTION' in the sender DB"
( cd "$REPO" && "$BIN" --format line --db "$SENDER_DB" \
    tour create "$COLLECTION" --description "p2p smoke test" >/dev/null )

echo "==> Starting provider (push --p2p) in the background"
( cd "$REPO" && "$BIN" --format line --db "$SENDER_DB" \
    tour push --p2p "$COLLECTION" ) >"$PUSH_OUT" 2>&1 &
PUSH_PID=$!

echo "==> Waiting for the ticket file path in provider output"
TICKET_FILE=""
for _ in $(seq 1 60); do
  if ! kill -0 "$PUSH_PID" 2>/dev/null; then
    echo "!! provider exited early; output was:" >&2
    cat "$PUSH_OUT" >&2
    exit 1
  fi
  TICKET_FILE="$(grep -o '/[^ ]*codemark-ticket-[^ ]*\.txt' "$PUSH_OUT" | head -1 || true)"
  [ -n "$TICKET_FILE" ] && [ -f "$TICKET_FILE" ] && break
  sleep 0.5
done

if [ -z "$TICKET_FILE" ] || [ ! -f "$TICKET_FILE" ]; then
  echo "!! never saw a ticket file; provider output was:" >&2
  cat "$PUSH_OUT" >&2
  exit 1
fi
echo "    ticket file: $TICKET_FILE ($(wc -c <"$TICKET_FILE" | tr -d ' ') bytes)"

echo "==> Pulling into the receiver DB (60s timeout)"
timeout 60 "$BIN" --format line --db "$RECEIVER_DB" \
  tour pull --p2p "$TICKET_FILE"

echo "==> Asserting the collection landed in the receiver DB"
if "$BIN" --format line --db "$RECEIVER_DB" tour show "$COLLECTION" >/dev/null 2>&1; then
  echo "PASS: '$COLLECTION' imported over p2p on a single machine"
else
  echo "!! FAIL: '$COLLECTION' not found in receiver DB" >&2
  exit 1
fi
