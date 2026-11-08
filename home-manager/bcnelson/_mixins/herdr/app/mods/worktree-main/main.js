// New worktree from the repo's default branch, not from HEAD.
//
// The shipped `worktree` mod starts a new branch from wherever the space is
// (`git worktree add -b <branch> <path> HEAD`), which is rarely what you want
// when you are already on a feature branch. This one fetches the default
// branch first and bases the new branch on the remote's copy of it.
//
// Opened with `mod.open:worktree-main` (keys.json), so it gets no props but
// its overlay: the repo is the focused terminal's folder, on the active
// space's machine.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { run, props, closeOverlay, theme, spaces, panes, focused, exec, home } from "herdr";

// Where a space is now: its first terminal's live folder (as the shipped mods do).
function place(s) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd || null;
}

// Same place as the shipped mod, so the spaces sidebar treats both alike.
const ROOT = (userHome) => `${userHome}/.herdr/worktrees`;
const slug = (branch) => branch.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
const tilde = (path, userHome) => (userHome && path.startsWith(userHome) ? `~${path.slice(userHome.length)}` : path);

async function git(cwd, args, on) {
  const r = await exec("git", args, { cwd, on });
  return { ok: r.code === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? r.stdout ?? "").trim() };
}

// The repo's default branch as { remote, branch }: origin/HEAD when the clone
// recorded it, else a local main or master (remote null when there is no
// origin to fetch from).
async function defaultBranch(top, on) {
  const head = await git(top, ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], on);
  if (head.ok && head.out.startsWith("origin/")) return { remote: "origin", branch: head.out.slice(7) };
  const hasOrigin = (await git(top, ["remote", "get-url", "origin"], on)).ok;
  for (const b of ["main", "master"]) {
    const local = await git(top, ["show-ref", "--verify", "--quiet", `refs/heads/${b}`], on);
    const remote = hasOrigin && (await git(top, ["show-ref", "--verify", "--quiet", `refs/remotes/origin/${b}`], on));
    if (remote?.ok) return { remote: "origin", branch: b };
    if (local.ok) return { remote: null, branch: b };
  }
  return null;
}

export default class WorktreeMain extends View {
  init(_props, cx) {
    // From a space's menu: that space, where its first terminal is now.
    // From the key: the active space, at the focused terminal's folder.
    const given = props()?.space && spaces().find((s) => s.id === props().space);
    const space = given || spaces().find((s) => s.active);
    this.on = space?.machine ?? null;
    this.folder = (given ? place(given) : focused()?.cwd) || space?.folder || null;
    this.userHome = this.on ? null : home();
    this.text = "";
    this.busy = false;
    this.error = "";
    this.repo = undefined; // undefined: looking, null: not usable (error says why)
    this.input = InputState.new({ value: "", placeholder: "new branch name" });
    this.input.on("change", (_e, cx) => { this.text = this.input.value(); this.error = ""; cx.notify(); });
    this.input.on("submit", (_e, cx) => this.go(cx));
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();

    cx.spawn(async (cx) => {
      try {
        if (this.on) {
          const r = await exec("sh", ["-c", 'printf %s "$HOME"'], { on: this.on });
          if (r.code !== 0 || !r.stdout.trim()) throw new Error("Could not find the remote home folder.");
          this.userHome = r.stdout.trim();
        }
        this.folder ||= this.userHome;
        const top = await git(this.folder, ["rev-parse", "--show-toplevel"], this.on);
        const common = await git(this.folder, ["rev-parse", "--path-format=absolute", "--git-common-dir"], this.on);
        if (!top.ok || !common.ok) throw new Error(`${tilde(this.folder, this.userHome)} is not in a git repository.`);
        const main = common.out.endsWith("/.git") ? common.out.slice(0, -5) : common.out;
        const base = await defaultBranch(top.out, this.on);
        if (!base) throw new Error("Couldn't find a default branch (no origin/HEAD, main or master).");
        this.repo = { top: top.out, main, name: main.split("/").pop(), base };
      } catch (error) {
        this.repo = null;
        this.error = String(error?.message ?? error);
      }
      cx.notify();
    });
  }

  close() {
    closeOverlay(props().overlay);
  }

  go(cx) {
    if (this.busy || !this.repo) return;
    cx.spawn(async (cx) => {
      this.busy = true;
      this.error = "";
      cx.notify();
      try { this.error = await this.create(); }
      catch (error) { this.error = String(error?.message ?? error); }
      this.busy = false;
      if (!this.error) this.close();
      cx.notify();
    });
  }

  async create() {
    const branch = this.text.trim();
    if (!branch) return "Type a branch name.";
    if (!(await git(this.repo.top, ["check-ref-format", "--branch", branch], this.on)).ok) return `"${branch}" is not a valid branch name.`;
    // An existing branch would not be "from main"; the shipped New/Open
    // worktree handle that case.
    if ((await git(this.repo.main, ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`], this.on)).ok)
      return `Branch ${branch} already exists. Use Open worktree or New worktree instead.`;

    const { remote, branch: b } = this.repo.base;
    let start = b;
    if (remote) {
      // Fetch just the default branch so the base is current. Offline, carry
      // on from the last fetched copy rather than refusing.
      await git(this.repo.top, ["fetch", "--quiet", remote, b], this.on);
      start = `${remote}/${b}`;
    }

    const path = `${ROOT(this.userHome)}/${this.repo.name}/${slug(branch)}`;
    // Starting from origin/main also makes it the upstream (git's default), so
    // status and `git pull` follow main; push.default=current still pushes to
    // a branch of the same name.
    const r = await git(this.repo.top, ["worktree", "add", "-b", branch, path, start], this.on);
    if (!r.ok) return r.err || "git worktree add failed.";
    // Unnamed, like the shipped mod: the space follows its folder and nests under the repo.
    run("space.new", { folder: path, machine: this.on });
    return "";
  }

  render() {
    const t = theme();
    const r = this.repo;
    const base = r ? (r.base.remote ? `${r.base.remote}/${r.base.branch}` : r.base.branch) : "";
    const detail =
      r === undefined
        ? "…"
        : r === null
          ? ""
          : `A new branch from ${base}${r.base.remote ? " (fetched first)" : ""}, checked out in ${tilde(ROOT(this.userHome), this.userHome)}/${r.name}/ as a new space.`;
    return v_flex()
      .id("worktree-main")
      .track_focus(this.handle)
      .on_key_down((e) => { if (e.key === "escape") this.close(); })
      .size_full()
      .p(16)
      .gap(10)
      .bg(t.popover)
      .text_size(13)
      .child(div().font_semibold().text_color(t.foreground).child(`New worktree from ${base || "main"}${r ? ` of ${r.name}` : ""}`))
      .when(Boolean(detail), (el) => el.child(div().text_color(t.muted).child(detail)))
      .when(Boolean(r), (el) =>
        el.child(
          Input.new(this.input)
            .h(32)
            .px(10)
            .items_center()
            .rounded(6)
            .border(1)
            .border_color(t.accent)
            .bg(t.background)
            .text_color(t.foreground),
        ),
      )
      .when(Boolean(this.error), (el) => el.child(div().text_color(t.blocked).child(this.error)))
      .child(div().flex_1())
      .child(
        h_flex()
          .justify_end()
          .text_size(11)
          .text_color(t.dim)
          .child(this.busy ? "fetching and creating…" : r ? "↵ create · esc cancel" : "esc close"),
      );
  }
}
