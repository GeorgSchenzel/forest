//! Queries and operations on one project's bare repo.

use std::path::{Path, PathBuf};

use crate::error::{Result, fail};
use crate::git::{git, git_loud, git_ok, git_out, relative_paths};
use crate::layout::canonical;

pub struct Worktree {
    pub path: PathBuf,
    /// `None` when detached.
    pub branch: Option<String>,
    /// git considers it prunable (its dir is gone).
    pub prunable: bool,
}

impl Worktree {
    pub fn dir_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The project's worktrees, without the bare repo itself.
pub fn worktrees(bare: &Path) -> Result<Vec<Worktree>> {
    let out = git(bare, ["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut is_bare = false;
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
            });
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
pub fn live_worktrees(bare: &Path) -> Result<Vec<Worktree>> {
    let list = worktrees(bare)?;
    if !list.iter().any(|w| w.prunable) {
        return Ok(list);
    }
    git(bare, ["worktree", "prune"])?;
    worktrees(bare)
}

/// `origin/HEAD` if it exists, else the bare repo's own `HEAD`. Never guesses.
pub fn default_branch(bare: &Path) -> Result<String> {
    if let Some(r) = git_out(
        bare,
        ["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) && let Some(b) = r.strip_prefix("refs/remotes/origin/")
    {
        return Ok(b.to_string());
    }
    git_out(bare, ["symbolic-ref", "--quiet", "HEAD"])
        .and_then(|r| r.strip_prefix("refs/heads/").map(String::from))
        .ok_or_else(|| fail!("cannot determine the default branch of {}", bare.display()))
}

pub fn origin_url(bare: &Path) -> Option<String> {
    git_out(bare, ["remote", "get-url", "origin"])
}

pub fn ref_exists(bare: &Path, refname: &str) -> bool {
    git_ok(bare, ["show-ref", "--verify", "--quiet", refname])
}

pub fn commit_exists(bare: &Path, rev: &str) -> bool {
    git_ok(
        bare,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

/// Fetches `origin`, and sets `origin/HEAD` if it is missing (remote added later).
pub fn fetch(bare: &Path) -> Result<()> {
    git_loud(bare, ["fetch", "--prune", "origin"])?;
    if !ref_exists(bare, "refs/remotes/origin/HEAD") {
        // Fails on an empty remote; the default then falls back to our own HEAD.
        let _ = git_out(bare, ["remote", "set-head", "origin", "--auto"]);
    }
    Ok(())
}

/// `git worktree add`, with relative paths when the installed git supports them.
pub fn worktree_add(bare: &Path, args: &[&str], path: &Path, rest: &[&str]) -> Result<()> {
    let mut all: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "add".as_ref()];
    if relative_paths() {
        all.push("--relative-paths".as_ref());
    }
    all.extend(args.iter().map(std::ffi::OsStr::new));
    all.push(path.as_os_str());
    all.extend(rest.iter().map(std::ffi::OsStr::new));
    git(bare, all).map(|_| ())
}

/// Uncommitted changes or untracked, non-ignored files.
pub fn is_dirty(worktree: &Path) -> Result<bool> {
    Ok(!git(worktree, ["status", "--porcelain"])?.is_empty())
}

/// Dropping `branch` is safe when its upstream is gone, or every commit is on a
/// remote-tracking ref or on the local default branch.
pub fn branch_is_safe_to_delete(bare: &Path, branch: &str, default: &str) -> bool {
    let remote = git_out(bare, ["config", &format!("branch.{branch}.remote")]);
    let merge = git_out(bare, ["config", &format!("branch.{branch}.merge")]);
    if let (Some(remote), Some(merge)) = (remote, merge)
        && remote != "."
        && let Some(upstream) = merge.strip_prefix("refs/heads/")
        && !ref_exists(bare, &format!("refs/remotes/{remote}/{upstream}"))
    {
        return true;
    }
    let local = format!("refs/heads/{branch}");
    let default_ref = format!("refs/heads/{default}");
    let mut args = vec!["rev-list", "--count", &local, "--not", "--remotes"];
    if default != branch && commit_exists(bare, &default_ref) {
        args.push(&default_ref);
    }
    git_out(bare, args).is_some_and(|n| n == "0")
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
