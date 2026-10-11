# forest

Work on many branches at once, in one repo or across several, without thinking about where anything lives.

> Personal and experimental. It fits my workflow and may not fit yours.

## What you get

- **Parallel work without switching.** Every branch gets its own folder. You can review a PR, fix a bug and keep building a feature at the same time, with no stashing and no rebuilds after a checkout.
- **One command to start, one to finish.** `forest new` gives you a ready folder for a branch. `forest rm` cleans it up again, and refuses if you would lose work.
- **Everything has a known place.** Branches live at predictable paths, so you never search for "that checkout from last week". `forest ls` shows all of them with their status.
- **Branches across repos.** When a change spans several repos, forest gives you one folder with the same branch checked out in each of them.

## Forests

A forest is a named group of repos that you often change together, for example a backend and its frontend:

```sh
forest define shop backend frontend
```

`forest new shop:feat/y` then creates branch `feat/y` in every repo of `shop` and puts them side by side in one folder:

```
_forests/shop/feat-y/
  backend/     # on feat/y
  frontend/    # on feat/y
```

You work in that folder as one workspace: open it in your editor, run both services, make one change across both repos. Each repo still commits and pushes on its own, as usual. When the feature is done, `forest rm shop:feat/y` removes the whole thing.

## Layout

All repos live under one root (`~/projects`, or `$FOREST_ROOT`):

```
~/projects/
  backend/
    main/
    feat-x/          # branch feat/x
  frontend/
    main/
  _forests/
    shop/
      feat-y/
        backend/
        frontend/
```

The layout is fixed on purpose, so there is nothing to configure except which repos belong to a forest.

## Usage

```sh
forest clone git@github.com:me/backend.git   # or: forest init <name> for a new repo
forest clone git@github.com:me/frontend.git

forest new backend:feat/x                    # one repo
cd "$(forest path backend:feat/x)"

forest define shop backend frontend          # group repos into a forest
forest new shop:feat/y                       # same branch in all of them

forest ls                                    # everything, with dirty / ahead / behind
forest rm shop:feat/y --delete-branch        # clean up worktrees and branches
```

You address a target as `<project-or-forest>:<branch>`. Inside a project or forest folder, the branch name alone is enough. `path` and `rm` with no argument act on the worktree you are in.

| Command | Purpose |
|---|---|
| `init <name> [--branch <b>]` | New project with no remote |
| `clone <url> [name]` | New project from a remote, or from a local repo |
| `migrate <path> [name]` | Move an existing clone or bare repo into the layout |
| `define <forest> [projects…] [--add/--remove <p>…]` | Create a forest or change its projects |
| `new <target> [--from <ref>]` | Create a worktree, or one per project of a forest. Re-running fills any gaps |
| `ls [name]` | List projects, forests and worktrees with their status |
| `path [target]` | Print the absolute path |
| `rm [target] [--force] [--delete-branch]` | Remove worktrees, and optionally their branches |
| `prune` | Clean up stale worktree metadata |

Global flags: `--json` (versioned output for scripts) and `--root <path>`.

With `--json`, `path`, `new` and `rm` describe their target the same way however it was given:

```json
{"v":1,"kind":"workforest","name":"shop","dir":"feat-y","branch":"feat/y","path":"/Users/me/projects/_forests/shop/feat-y"}
```

`kind` is `worktree` or `workforest`, `name` is the project or forest, and (`name`, `dir`) is unique. `branch` is `null` for a detached worktree, or a workforest with no worktree on a branch.

## Good to know

- Branch folders are flat, so `feat/x` becomes `feat-x`. `ls` shows the real branch name.
- `new` uses an existing local branch, else the remote branch (with tracking), else creates a new branch from `--from` or the default branch.
- Git allows a branch to be checked out in only one worktree. This means a forest can't check out `main`, because each project's `main/` folder already holds it.
- `rm` never loses commits. It refuses if there are uncommitted changes or if you target a default-branch worktree. `--delete-branch` only deletes a branch that is merged, pushed, or whose upstream is gone.
- `migrate` moves the working tree instead of checking it out again, so staged, untracked and ignored files survive. Other worktrees are moved to their branch folders. It refuses a detached HEAD or an operation in progress, and a repo outside the root must be on the same filesystem.
- Exit codes: `0` ok, `1` error, `2` refused by a safety check.

## Install

```sh
just install     # cargo install --path . --locked --force
just uninstall
```
