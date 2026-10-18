// Runs scripts/agent-guard before every tool call; see AGENTS.md "Guardrails".
// Loaded once this checkout is trusted (pi project trust).
// @ts-nocheck
import { spawnSync } from "node:child_process";
import path from "node:path";

export default function (pi) {
  pi.on("tool_call", async (event, ctx) => {
    const root = spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd: ctx?.cwd ?? process.cwd(), encoding: "utf8" });
    const guard = path.join(root.stdout.trim() || process.cwd(), "scripts", "agent-guard");
    const r = spawnSync(guard, ["pi"], {
      input: JSON.stringify({ tool_name: event.toolName, tool_input: event.input }),
      encoding: "utf8",
      timeout: 60_000,
    });
    if (r.status === 2) {
      let reason = (r.stderr || "").trim();
      try {
        reason = JSON.parse(r.stdout).message || reason;
      } catch {}
      return { block: true, reason: reason || "blocked by the nix-config guard" };
    }
  });
}
