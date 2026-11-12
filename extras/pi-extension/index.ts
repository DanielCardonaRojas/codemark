// Codemark live agent bridge for pi (experimental).
//
// On session start this spawns `codemark agent watch`, which registers a pi
// session in the repo's codemark database and streams replies to threads the
// session owns as JSON lines. Each reply is handed to pi as a follow-up user
// message; pi answers with the `codemark_reply` tool, and the answer appears
// in the codemark TUI live.
//
// Install: copy this directory to ~/.pi/agent/extensions/codemark/ (or the
// project's .pi/extensions/). Set CODEMARK_BIN if `codemark` is not on PATH.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { spawn, type ChildProcess } from "node:child_process";

const CODEMARK = process.env.CODEMARK_BIN ?? "codemark";

interface ReplyEvent {
  type: "reply";
  delivery_id: string;
  thread_id: string;
  author: string;
  body: string;
  bookmark: { id: string; file: string };
  anchor_hint?: string | null;
  thread: { author: string; body: string }[];
}

function render(e: ReplyEvent): string {
  const where = e.anchor_hint ? ` (${e.anchor_hint})` : "";
  const history = e.thread.map((t) => `> ${t.author}: ${t.body.replace(/\n/g, "\n>   ")}`).join("\n");
  return [
    `[codemark] ${e.author} replied in a comment thread you own on \`${e.bookmark.file}\`${where}:`,
    "",
    e.body,
    ...(history ? ["", "Thread so far:", history] : []),
    "",
    `Answer with the codemark_reply tool, thread_id "${e.thread_id}".`,
  ].join("\n");
}

export default function (pi: ExtensionAPI) {
  let child: ChildProcess | undefined;
  let sessionId: string | undefined;
  let cwd = process.cwd();

  pi.on("session_start", async (_event, ctx) => {
    cwd = ctx.cwd;
    child = spawn(CODEMARK, ["agent", "watch", "--kind", "pi", "--label", "pi"], {
      cwd,
      stdio: ["ignore", "pipe", "inherit"],
    });
    let buffer = "";
    child.stdout!.setEncoding("utf8");
    child.stdout!.on("data", (chunk: string) => {
      buffer += chunk;
      let nl: number;
      // Split on LF only: JSON lines never contain a raw newline.
      while ((nl = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, nl);
        buffer = buffer.slice(nl + 1);
        if (!line.trim()) continue;
        let msg: any;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg.type === "session") sessionId = msg.session.id;
        else if (msg.type === "reply") pi.sendUserMessage(render(msg), { deliverAs: "followUp" });
      }
    });
    child.on("exit", () => {
      child = undefined;
    });
  });

  pi.on("session_shutdown", async () => {
    child?.kill();
    child = undefined;
    sessionId = undefined;
  });

  const run = async (args: string[], signal?: AbortSignal) => {
    if (!sessionId) throw new Error("codemark bridge is not connected (is codemark initialised here?)");
    const r = await pi.exec(CODEMARK, [...args, "--created-by", "pi", "--session", sessionId], { cwd, signal });
    if (r.code !== 0) throw new Error(r.stderr || r.stdout);
    return { content: [{ type: "text" as const, text: r.stdout }], details: {} };
  };

  pi.registerTool({
    name: "codemark_reply",
    label: "Codemark reply",
    description: "Answer a codemark comment thread you own. Use the thread_id from the [codemark] message.",
    parameters: Type.Object({ thread_id: Type.String(), body: Type.String() }),
    async execute(_id, params, signal) {
      return run(["comment", "reply", params.thread_id, "--body", params.body], signal);
    },
  });

  pi.registerTool({
    name: "codemark_comment",
    label: "Codemark comment",
    description:
      "Comment on a codemark bookmark, optionally on one node inside it (range: 42, 42-67 or 10:5-12:20), as a live thread the user can answer.",
    parameters: Type.Object({
      bookmark_id: Type.String(),
      body: Type.String(),
      range: Type.Optional(Type.String()),
    }),
    async execute(_id, params, signal) {
      const range = params.range ? ["--range", params.range] : [];
      return run(["comment", "add", params.bookmark_id, "--body", params.body, ...range], signal);
    },
  });
}
