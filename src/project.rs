//! Queries and operations on one repo, and on the worktrees of all repos.

use std::path::{Path, PathBuf};

use crate::error::{Result, fail};
use crate::git::{git, git_loud, git_ok, git_out, relative_paths};
use crate::layout::{Layout, canonical};

pub struct Worktree {
    pub path: PathBuf,
    /// `None` when detached.
    pub branch: Option<String>,
    /// git considers it prunable (its dir is gone).
    pub prunable: bool,
    /// The repo's own checkout, its home.
    pub home: bool,
}

/// The repo's worktrees, its home first.
pub fn worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let out = git(repo, ["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut is_bare = false;
    let mut first = true;
    for line in out.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(w) = current.take()
                && !is_bare
            {
                list.push(w);
            }
            is_bare = false;
        } else if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(Worktree {
                path: canonical(Path::new(p)),
                branch: None,
                prunable: false,
                home: first,
            });
            first = false;
        } else if let Some(w) = current.as_mut() {
            if line == "bare" {
                is_bare = true;
            } else if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if line.starts_with("prunable") {
                w.prunable = true;
            }
        }
    }
    Ok(list)
}

/// Like `worktrees`, but first prunes registrations whose dir is gone, so they
/// don't keep holding their path or branch.
pub fn live_worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let list = worktrees(repo)?;
    if !list.iter().any(|w| w.prunable) {
        return Ok(list);
    }
    git(repo, ["worktree", "prune"])?;
    worktrees(repo)
}

/// Every worktree of every repo, with its repo's name.
pub fn all_worktrees(layout: &Layout) -> Result<Vec<(String, Worktree)>> {
    let mut all = Vec::new();
    for p in layout.projects()? {
        for w in worktrees(&layout.repo_dir(&p))? {
            all.push((p.clone(), w));
        }
    }
    Ok(all)
}

/// The worktrees in a forest, in repo order.
pub fn members(layout: &Layout, forest: &str) -> Result<Vec<(String, Worktree)>> {
    let mut all = all_worktrees(layout)?;
    all.retain(|(_, w)| layout.forest_of(&w.path).as_deref() == Some(forest));
    Ok(all)
}

/// The forests with a worktree on `branch`.
pub fn forests_on(all: &[(String, Worktree)], layout: &Layout, branch: &str) -> Vec<String> {
    let mut found: Vec<String> = all
        .iter()
        .filter(|(_, w)| w.branch.as_deref() == Some(branch))
        .filter_map(|(_, w)| layout.forest_of(&w.path))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// `origin/HEAD` if it exists, else the branch of the repo's `HEAD`. Never guesses.
pub fn default_branch(repo: &Path) -> Result<String> {
    if let Some(r) = git_out(
        repo,
        ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) && let Some(b) = r.strip_prefix("refs/remotes/origin/")
    {
        return Ok(b.to_string());
    }
    git_out(repo, ["symbolic-ref", "--quiet", "HEAD"])
        .and_then(|r| r.strip_prefix("refs/heads/").map(String::from))
        .ok_or_else(|| fail!("cannot determine the default branch of {}", repo.display()))
}

pub fn origin_url(repo: &Path) -> Option<String> {
    git_out(repo, ["remote", "get-url", "origin"])
}

pub fn ref_exists(repo: &Path, refname: &str) -> bool {
    git_ok(repo, ["show-ref", "--verify", "--quiet", refname])
}

pub fn commit_exists(repo: &Path, rev: &str) -> bool {
    git_ok(
        repo,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

/// Whether `ancestor` is an ancestor of `descendant` (a branch is its own
/// ancestor). Also false when either ref doesn't exist.
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    git_ok(repo, ["merge-base", "--is-ancestor", ancestor, descendant])
}

/// Fetches `origin`, and sets `origin/HEAD` if it is missing (remote added later).
pub fn fetch(repo: &Path) -> Result<()> {
    git_loud(repo, ["fetch", "--prune", "origin"])?;
    if !ref_exists(repo, "refs/remotes/origin/HEAD") {
        // Fails on an empty remote; the default then falls back to our own HEAD.
        let _ = git_out(repo, ["remote", "set-head", "origin", "--auto"]);
    }
    Ok(())
}

/// Makes git write relative worktree paths, when the installed git supports them.
pub fn configure(repo: &Path) -> Result<()> {
    if relative_paths() {
        git(repo, ["config", "worktree.useRelativePaths", "true"])?;
    }
    Ok(())
}

/// `git worktree add`, with relative paths when the installed git supports them.
pub fn worktree_add(repo: &Path, args: &[&str], path: &Path, rest: &[&str]) -> Result<()> {
    let mut all: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "add".as_ref()];
    if relative_paths() {
        all.push("--relative-paths".as_ref());
    }
    all.extend(args.iter().map(std::ffi::OsStr::new));
    all.push(path.as_os_str());
    all.extend(rest.iter().map(std::ffi::OsStr::new));
    git(repo, all).map(|_| ())
}

/// `git worktree repair` for worktrees that were moved by hand.
pub fn worktree_repair(repo: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut args = vec!["worktree".to_string(), "repair".to_string()];
    if relative_paths() {
        args.push("--relative-paths".into());
    }
    args.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
    git(repo, args).map(|_| ())
}

/// Uncommitted changes or untracked, non-ignored files.
pub fn is_dirty(worktree: &Path) -> Result<bool> {
    Ok(!git(worktree, ["status", "--porcelain"])?.is_empty())
}

/// The refs a commit can be on so that deleting a branch loses nothing: every
/// remote-tracking ref, and the local default branch.
fn kept_refs(repo: &Path, branch: &str, default: &str) -> Vec<String> {
    let mut refs = vec!["--remotes".to_string()];
    let default_ref = format!("refs/heads/{default}");
    if default != branch && commit_exists(repo, &default_ref) {
        refs.push(default_ref);
    }
    refs
}

/// How many commits of `branch` are on none of `refs`.
fn commits_not_on(repo: &Path, branch: &str, refs: &[String]) -> Option<u32> {
    let local = format!("refs/heads/{branch}");
    let mut args = vec!["rev-list", "--count", &local, "--not"];
    args.extend(refs.iter().map(String::as_str));
    git_out(repo, args).and_then(|n| n.parse().ok())
}

/// Commits of `branch` that deleting it would lose: those on no remote-tracking
/// ref and not on the local default branch. `None` if that can't be told.
pub fn unsaved_commits(repo: &Path, branch: &str, default: &str) -> Option<u32> {
    commits_not_on(repo, branch, &kept_refs(repo, branch, default))
}

/// Whether `branch` had an upstream that no longer exists, as after a merged and
/// deleted MR.
pub fn upstream_gone(repo: &Path, branch: &str) -> bool {
    let remote = git_out(repo, ["config", &format!("branch.{branch}.remote")]);
    let merge = git_out(repo, ["config", &format!("branch.{branch}.merge")]);
    if let (Some(remote), Some(merge)) = (remote, merge)
        && remote != "."
        && let Some(upstream) = merge.strip_prefix("refs/heads/")
    {
        return !ref_exists(repo, &format!("refs/remotes/{remote}/{upstream}"));
    }
    false
}

/// Whether work on `branch` is done: its upstream is gone, or it was committed on
/// and every commit is now on the default branch. A branch nobody committed on
/// yet looks merged too, so its reflog must show it moved since it was created.
pub fn is_done(repo: &Path, branch: &str, default: &str) -> bool {
    if upstream_gone(repo, branch) {
        return true;
    }
    let mut on_default = vec![format!("refs/heads/{default}")];
    if ref_exists(repo, &format!("refs/remotes/origin/{default}")) {
        on_default.push(format!("refs/remotes/origin/{default}"));
    }
    on_default.retain(|r| commit_exists(repo, r));
    if branch == default || on_default.is_empty() {
        return false;
    }
    if commits_not_on(repo, branch, &on_default) != Some(0) {
        return false;
    }
    let local = format!("refs/heads/{branch}");
    let tip = git_out(repo, ["rev-parse", &local]);
    let created = git_out(repo, ["reflog", "show", "--format=%H", &local])
        .and_then(|log| log.lines().last().map(String::from));
    matches!((tip, created), (Some(t), Some(c)) if t != c)
}

/// Rejects names git would not accept as a branch.
pub fn check_branch_name(branch: &str) -> Result<()> {
    if git_ok(Path::new("/"), ["check-ref-format", "--branch", branch]) && !branch.starts_with('-')
    {
        Ok(())
    } else {
        Err(fail!("invalid branch name '{branch}'"))
    }
}
