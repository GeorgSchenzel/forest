# forest

Work on many branches at once, in one repo or across several, without thinking about where anything lives.

> Personal and experimental. It fits my workflow and may not fit yours.

## What you get

- **Parallel work without switching.** Every branch gets its own folder. You can review a PR, fix a bug and keep building a feature at the same time, with no stashing and no rebuilds after a checkout.
- **One command to start, one to finish.** `forest new` gives you a ready folder for a branch. `forest rm` cleans it up again, and refuses if you would lose work. `forest clean` clears away everything that is merged.
- **Everything has a known place.** Branches live at predictable paths, so you never search for "that checkout from last week". `forest ls` shows all of them with their status.
- **Branches across repos.** When a change spans several repos, the same branch of each sits side by side in one folder.

## Forests

A forest is the folder for one branch. It holds a checkout of that branch for each repo you work on in it:

```sh
forest new feat/auth backend            # forests/feat-auth/backend/
forest new feat/auth frontend           # forests/feat-auth/frontend/, added later
```

You work in that folder as one workspace: open it in your editor, run both services, make one change across both repos. Each repo still commits and pushes on its own, as usual. When the feature is done, `forest rm feat/auth` removes the whole thing.

A forest is named after its branch. `--as` gives a new forest another name, when the branch says too little:

```sh
forest new cancellation tuna-agent --as tuna-cancel
```

Both the name and the branch address it afterwards, and `forest mv` renames it. Nothing is stored besides the folders: a forest's repos are the worktrees in it.

## Layout

All repos live under one root (`~/projects`, or `$FOREST_ROOT`):

```
~/projects/
  repos/
    backend/         # a normal clone, on the default branch
    frontend/
  forests/
    feat-auth/       # branch feat/auth
      backend/
      frontend/
    tuna-cancel/     # branch cancellation
      tuna-agent/
```

The layout is fixed on purpose, so there is nothing to configure. Other folders in the root are left alone.

## Usage

```sh
forest clone git@github.com:me/backend.git   # or: forest init <name> for a new repo
forest clone git@github.com:me/frontend.git

forest new feat/x backend                    # one repo
fcd feat/x                                   # into forests/feat-x/backend

forest new feat/auth backend frontend        # the same branch in several repos

forest ls                                    # everything, with dirty / ahead / behind
forest rm feat/auth --delete-branch          # clean up worktrees and branches
forest clean                                 # remove every forest that is done
```

`path` of a forest with only one repo is that repo's worktree, so you land where git is.

A target is a forest, by its name or its branch, or a repo, which means its own checkout. A repo after it picks one worktree: `forest path feat/auth backend`, or `forest path main backend` for the repo's checkout. Names match exactly, and a word that names two things is an error, never a guess. `path` and `rm` with no argument act on the forest or repo you are in.

| Command | Purpose |
|---|---|
| `init <name> [--branch <b>]` | New repo with no remote |
| `clone <url> [name]` | New repo from a remote, or from a local repo |
| `migrate <path> [name]` | Move an existing clone, bare repo or older layout into the layout |
| `new <branch> <repo>… [--as <name>] [--from <ref>]` | Check out a branch in repos, in its forest |
| `ls [target]` | List repos and forests with their status |
| `path [target [repo]]` | Print the absolute path |
| `rm [target [repo]] [--force] [--delete-branch]` | Remove a forest or one of its worktrees, and optionally their branches |
| `mv <forest> <name>` | Rename a forest |
| `clean [--dry-run] [--delete-branch]` | Remove every forest whose work is done |
| `prune` | Clean up stale worktree metadata |
| `shell-init zsh` | Print the shell code for `fcd` and completion |

Global flags: `--json` (versioned output for scripts) and `--root <path>`.

With `--json`, `path`, `new`, `rm` and `mv` describe their target the same way however it was given:

```json
{"v":1,"kind":"forest","forest":"feat-auth","project":null,"branch":"feat/auth","path":"/Users/me/projects/forests/feat-auth"}
```

`kind` is `forest` or `worktree`. A worktree has its `project`, and its `forest` unless it is the repo's own checkout. `branch` is `null` for a detached worktree, or a forest with no worktree on a branch.

## Shell

A program can't change your shell's directory, so `cd` needs a shell function. Add to `~/.zshrc`, anywhere:

```sh
command -v forest >/dev/null && eval "$(forest shell-init zsh)"
```

That gives `fcd [target [repo]]`, which goes where `forest path` points. `forest` and `fcd` complete forests, branches and repos. The shell code only calls `forest`, so a new build takes effect in open shells.

## Good to know

- Forest folders are flat, so `feat/x` becomes `feat-x`. `ls` shows the real branch name when it differs.
- `new` adds to the forest that has the branch already, else makes one. It fails if a worktree is there already, and is all or nothing: if one repo fails, the worktrees and branches it made are removed again.
- `new` uses an existing local branch, else the remote branch (with tracking), else creates a new branch from `--from` or the default branch. It says which on stderr, and as `branch_created` and `from` with `--json`. `--from` is only for a new branch, and must exist in every repo.
- Git allows a branch to be checked out in only one worktree. A repo's own checkout holds its default branch, so no forest can have it.
- `rm` never removes a repo's own checkout. It refuses if there are uncommitted changes. `--delete-branch` only deletes a branch if every commit is on a remote-tracking branch or on the default branch, and never the default branch itself.
- `clean` removes a forest when none of its worktrees has uncommitted changes, and each branch is merged into the default branch or its upstream is gone. A branch nobody committed on yet doesn't count as merged. Ignored files go with the forest, as with `rm`. Branches stay, unless `--delete-branch`, which follows the same rule as `rm`: a squash-merged branch with commits that were never pushed is kept.
- `mv` renames only the folder, and tells git where the worktrees are now. Renaming a folder by hand breaks that link; `git worktree repair` in the repo fixes it.
- `migrate` moves working trees instead of checking them out again, so staged, untracked and ignored files survive. For a bare repo, the worktree on the default branch becomes the repo's checkout, and the others move to their forests. Worktrees inside the repo stay where they are. It refuses an operation in progress in the worktree that becomes the checkout, and a repo outside the root must be on the same filesystem.
- Exit codes: `0` ok, `1` error, `2` refused by a safety check.

## Install

```sh
just install     # cargo install --path . --locked --force
just uninstall
```
