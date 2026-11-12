# Codemark bridge for pi (experimental)

Replies to comment threads pi started in codemark are pushed into the running
pi session, and pi's answers appear in the codemark TUI live.

```sh
mkdir -p ~/.pi/agent/extensions
cp -r extras/pi-extension ~/.pi/agent/extensions/codemark
```

Then, in a repo with a codemark database, start `pi`. The extension runs
`codemark agent watch --kind pi`, which registers a session. Threads pi opens
with the `codemark_comment` tool are owned by that session; a reply to one in
the TUI is delivered to pi as a follow-up message, and pi answers with
`codemark_reply`.

Set `CODEMARK_BIN` if `codemark` is not on `PATH`. Untested against a live pi
build: it follows the documented extension API (`sendUserMessage`,
`registerTool`, `session_start` / `session_shutdown`, `exec`).
