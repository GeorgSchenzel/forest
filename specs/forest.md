# forest: worktree & workforest manager

Status: draft, experimental. Personal and opinionated to one workflow, not meant for publishing. forest knows nothing about zellij, sandboxes or agents, and no other tool depends on it. Deferred features are kept outside this repo, in the ideas file next to the mise specs.

## 1. Goal

A fixed, predictable layout for bare repos, git worktrees and multi-project workforests, so starting work on a branch, in one project or across several, is one command and finding it again needs no thought.

**Principles**
- Convention over configuration. The layout is fixed. The only config is a forest's project list, and the root comes from `FOREST_ROOT` (default `~/projects`).
- Only git is required.
- KISS / YAGNI. v1 does the minimum, and edge cases fail with a clear error instead of being handled cleverly.
- Every command has `--json` output, for scripts.

## 2. Concepts & layout

| Concept | Meaning |
|---|---|
| **Root** | `$FOREST_ROOT`, default `~/projects` |
| **Project** | A bare repo at `<root>/<project>/.bare`, with or without a remote |
| **Worktree** | One branch of one project: `<root>/<project>/<branch-dir>` |
| **Forest** | A named set of projects, defined in `<root>/_forests/<forest>/.forest.toml` |
| **Workforest** | One branch across a forest's projects: `<root>/_forests/<forest>/<branch-dir>/<project>` |

```
~/projects/
  backend/
    .bare/
    main/
    feat-x/                   # branch feat/x
  frontend/
    .bare/
    main/
  _forests/
    shop/
      .forest.toml            # projects = ["backend", "frontend"]
      feat-y/
        backend/              # worktree of backend/.bare
        frontend/             # worktree of frontend/.bare
```

A forest is created with `forest define shop backend frontend`, which writes its `.forest.toml`:

```toml
projects = ["backend", "frontend"]
```

The file can also be edited by hand.

**Names:** a project name must not start with `_` or `.`, which keeps `_forests` (and any future forest-owned dir) out of the project namespace and sorts it first in `ls`. `_forests` is not hidden, so `ls`, `fd` and fuzzy finders still see workforests. A forest name must not equal a project name, so a name always refers to exactly one of them.

## 3. Addressing & cwd detection

A target is `<name>:<branch>`, where `<name>` is a project or a forest: `backend:feat/x`, `shop:feat/y`. git forbids `:` in branch names, so the split is never ambiguous. `<branch>` matches the real branch name or its dir name.

**Inference from cwd:** anywhere under `<root>/<project>/` (the project dir, any worktree, any subdir of either), the context is that project. Anywhere under `<root>/_forests/<forest>/`, including inside a workforest's project worktree, the context is that forest.

- A bare argument is always a branch in the current context: inside `backend/`, `forest new feat/x` means `backend:feat/x`.
- With no argument, `path` and `rm` act on the worktree or workforest cwd is in. Inside a workforest, that is the whole workforest.
- Outside any context, a bare branch is an error that asks for `<name>:<branch>`.

## 4. Commands

| Command | Behavior |
|---|---|
| `forest init <name> [--branch <b>]` | Start a project with no remote: `git init --bare` into `<root>/<name>/.bare` (default branch `--branch`, else git's `init.defaultBranch`) and add the default-branch worktree on the unborn branch (`worktree add --orphan`). |
| `forest clone <url> [name]` | Not `git clone --bare` (§5, Remotes). Runs `git init --bare`, `remote add origin <url>`, `fetch`, `remote set-head origin --auto`, points the bare repo's `HEAD` at the default branch (otherwise it keeps `git init`'s default, which may not exist), then adds the default-branch worktree tracking `origin/<default>`. `<url>` can be a local path, e.g. an existing normal clone. |
| `forest migrate <path> [name]` | Move an existing repo into the layout as `<root>/<name>` (name defaults to the dir name without `.git`), see §5. Re-running on a project that is already in the layout does nothing. |
| `forest define <forest> [project...] [--add <p>...] [--remove <p>...]` | Create a forest from existing projects, or change an existing forest's project list with `--add` / `--remove`. Removing a project keeps its worktrees in existing workforests. |
| `forest new <target> [--from <ref>]` | **Project:** create a worktree. Branch resolution: local branch, else remote branch (tracking set up), else a new branch from `--from` or the default branch. **Forest:** the same for every project of the forest, into `<root>/_forests/<forest>/<branch-dir>/<project>`. `--from` is used where the ref exists, else that project's default branch. Idempotent: what exists is kept and its path printed, so re-running fills the gaps (failed projects, projects added to the forest since). On partial failure, what was created stays and the exit code is 1. |
| `forest ls [name]` | Projects, forests, worktrees and workforests with branch, dir, dirty / ahead / behind / no-upstream. A project's listing includes its worktrees inside workforests. |
| `forest path [target]` | Print the absolute path (for `cd "$(forest path x)"` and scripts) |
| `forest rm [target] [--force] [--delete-branch]` | Remove a worktree or workforest (§5). |
| `forest prune` | `git worktree prune` for all projects, report orphaned dirs |

Global flags: `--json`, `--root <path>` (overrides `FOREST_ROOT`).

Exit codes: 0 ok, 1 error, 2 refused by a safety check (so scripts can tell them apart).

## 5. Behaviors

- **Default branch detection:** `origin/HEAD` if it exists, else the bare repo's own `HEAD` (set by `init` and `clone`). It never guesses `main` or `master`.
- **Branch dir names (flat slugs):** `/` → `-`, any other character outside `[A-Za-z0-9._-]` → `-`, and a leading `.` → `-` (so a dir never clashes with `.bare` or `.forest.toml`). A collision (`feat/x` vs `feat-x`) is an error, with no override: such branch pairs shouldn't exist. The real branch name is always kept, and `ls`/`--json` show both.
- **One branch, one worktree:** git allows a branch to be checked out in only one worktree, and forest doesn't work around it. `new` fails with an error naming the path that holds the branch. This is accepted, including its main consequence: a workforest can't use the default branch (`shop:main`), because each project's default-branch worktree already holds it.
- **Relative worktree paths:** use `worktree.useRelativePaths` when the installed git supports it (2.48+), so the layout can be moved and mounted elsewhere.
- **Remotes:** only `origin` is used. A project works the same with or without it, and gets it in one of two ways:
  - **`clone`:** the layout a normal `git clone` has: remote branches only as `origin/*`, the standard fetch refspec, and `origin/HEAD`. forest avoids `git clone --bare` because it copies every remote branch into a local branch with no upstream, sets no fetch refspec (so `fetch` updates no `origin/*` refs) and no `origin/HEAD`.
  - **Added later** to an `init` project, with plain git and no forest command: `git -C .bare remote add origin <url>` (which sets the standard refspec), then `git -C .bare push -u origin --all` (which pushes every branch and sets their upstreams).
  - **Without `origin`:** `new` skips the fetch, branches are resolved locally, `ls` shows every branch as no-upstream, and `--delete-branch` relies on the merged-into-default check.
- **Fetch:** when `origin` exists, `new` fetches the project before resolving branches (skip with `--offline`). If `origin/HEAD` is missing (the remote was added later), the fetch also runs `remote set-head origin --auto`.
- **Unborn default branch:** after `init`, the default branch has no commits. `ls` shows it with `head: null`, and `new` fails with an error asking for a first commit, since there is nothing to branch from.
- **`rm` safety:** removing a worktree never loses commits, because the branch is kept. So `rm` only refuses (exit 2) when:
  - the worktree has uncommitted changes or untracked, non-ignored files, or
  - it is a project's default-branch worktree.

  `--force` overrides both.
- **`--delete-branch`:** also deletes the local branch, but only if dropping it is safe: every commit is on a remote-tracking ref or on the local default branch, or its upstream is gone (deleted on the remote after a merge, the normal squash-merge flow). Otherwise it refuses, unless `--force`.
- **Workforest `rm`:** checks every project first, and any refusal refuses the whole `rm`. Then it removes every project worktree and the workforest dir. Entries in the workforest dir that aren't worktrees refuse the `rm`; with `--force`, the worktrees are removed but those entries are never deleted, so the dir stays and the exit code is 1.

- **`migrate`:** accepts a normal clone (`.git` dir + working tree), a bare repo (`foo.git`, with or without worktrees made inside it), or a nearly laid-out project (`.bare` present, worktrees in the wrong dirs, a `.git` file pointing at `.bare`). A repo outside the root is moved into it first (`rename`, so same filesystem only). Then:
  - The git dir becomes `<root>/<name>/.bare` (`core.bare = true`).
  - A normal clone's working tree is **moved**, not checked out again, to `<root>/<name>/<branch-dir>` for the checked-out branch, and keeps its index, so staged, unstaged, untracked and ignored files all survive.
  - Every other worktree outside `_forests` is moved to its branch dir (`git worktree move`); ones that can't be (detached, target exists, locked) are kept and reported, exit code 1. All links are repaired (`git worktree repair`), relative when supported.
  - With no worktree left, the default-branch worktree is added, as `clone` would.
  - It refuses (exit 2) a main worktree with a detached HEAD or an operation in progress (merge, rebase, cherry-pick, revert, bisect), and fails on submodules, on an unborn branch, and on a linked worktree (migrate the repo it belongs to instead).

## 6. JSON contract (sketch)

```json
{"v":1,"worktrees":[{"project":"backend","branch":"feat/x","dir":"feat-x",
  "forest":null,"path":"/home/georg/projects/backend/feat-x","head":"a1b2c3d",
  "dirty":false,"ahead":2,"behind":0,"upstream":"origin/feat/x"}]}
```

Workforest worktrees carry `"forest":"shop"`. The JSON is versioned with `v`, and fields are only added within a version.

## 7. Packaging

A standalone binary in its own repo. Nothing builds on it.
