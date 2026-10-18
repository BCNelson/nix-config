// Runs scripts/agent-guard before every tool call; see AGENTS.md "Guardrails".
// opencode has no command hooks, so the shared guard is called from a plugin:
// throwing from tool.execute.before cancels the call and shows the message.
export const AgentGuard = async ({ $, worktree, directory }) => {
  const guard = `${worktree || directory}/scripts/agent-guard`
  return {
    "tool.execute.before": async (input, output) => {
      const payload = JSON.stringify({ tool_name: input.tool, tool_input: output.args })
      const res = await $`${guard} opencode < ${new Response(payload)}`.nothrow().quiet()
      if (res.exitCode === 2) {
        let msg = res.stderr.toString().trim()
        try {
          msg = JSON.parse(res.stdout.toString()).message || msg
        } catch {}
        throw new Error(msg || "blocked by the nix-config guard")
      }
    },
  }
}
