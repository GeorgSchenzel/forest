//! `done`: merge a forest's branches into their repos' default branches, then
//! close it.
//!
//! Everything is checked first, all or nothing: worktrees clean and on branches,
//! homes on the default branch and clean, and every merge a fast-forward or
//! already in. Only then does anything happen, so a forest never gets stuck
//! half-merged.

use std::env;
use std::fs;
use std::path::Path;

use serde_json::json;

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::cmd::rm::{Item, forest_items, remove, require_worktree_target};
use crate::error::{Error, Result, fail};
use crate::git::git;
use crate::layout::{Layout, canonical};
use crate::project::{default_branch, is_ancestor, is_dirty};

/// How a branch meets its default branch.
enum How {
    /// Fast-forward the default branch, this one, to the branch.
    FastForward(String),
    /// The default branch, this one, already contains the branch.
    AlreadyIn(String),
}

impl How {
    fn default_branch(&self) -> &str {
        match self {
            How::FastForward(d) | How::AlreadyIn(d) => d,
        }
    }
}

pub fn done(
    layout: &Layout,
    target: Option<&str>,
    project: Option<&str>,
    delete_branch: bool,
) -> Result<Output> {
    let located = locate(layout, target, project)?;
    require_worktree_target(target, &located)?;
    let identity = located.identity(layout)?;
    // Before removing: a deleted cwd can't be resolved.
    let cwd_in_target = env::current_dir().is_ok_and(|c| canonical(&c).starts_with(&identity.path));
    let (items, orphans, forest) = match located {
        Located::Forest { name, dir } => {
            let (items, orphans) = forest_items(layout, &name, &dir)?;
            (items, orphans, Some(dir))
        }
        Located::Worktree {
            project, worktree, ..
        } => {
            if worktree.home {
                return Err(fail!(
                    "{} is the home of {project}; done merges the branches of a forest",
                    worktree.path.display()
                ));
            }
            let item = Item {
                missing: !worktree.path.exists(),
                path: worktree.path,
                branch: worktree.branch,
                project,
            };
            (vec![item], vec![], None)
        }
    };

    // Check everything first: any refusal refuses the whole forest.
    let mut refusals = Vec::new();
    for o in &orphans {
        refusals.push(format!("{} is not a worktree", o.display()));
    }
    let mut planned: Vec<(&Item, How)> = Vec::new();
    for item in &items {
        if let Some(how) = check(layout, item, delete_branch, &mut refusals)? {
            planned.push((item, how));
        }
    }
    if !refusals.is_empty() {
        return Err(Error::Refused(refusals.join("\n")));
    }

    // The merges are what can fail, so they go first; a failure leaves every
    // worktree in place, and `done` can simply be run again.
    let mut text = String::new();
    for (item, how) in &planned {
        let branch = item.branch.as_deref().unwrap_or_default();
        let p = &item.project;
        match how {
            How::FastForward(default) => {
                git(&layout.repo_dir(p), ["merge", "--ff-only", branch])
                    .map_err(|e| fail!("{p}: {e}"))?;
                text.push_str(&format!("merged {branch} into {default} of {p}\n"));
            }
            How::AlreadyIn(default) => {
                text.push_str(&format!("{branch} of {p} is already in {default}\n"));
            }
        }
    }

    let mut merged = Vec::new();
    let mut errors = Vec::new();
    for (item, how) in &planned {
        match close(layout, item, delete_branch) {
            Ok(branch_deleted) => {
                text.push_str(&format!("removed {}\n", item.path.display()));
                if branch_deleted && let Some(b) = &item.branch {
                    text.push_str(&format!("deleted branch {b} of {}\n", item.project));
                }
                merged.push(json!({
                    "project": item.project,
                    "branch": item.branch,
                    "into": how.default_branch(),
                    "already_merged": matches!(how, How::AlreadyIn(_)),
                    "path": item.path,
                    "branch_deleted": branch_deleted,
                }));
            }
            Err(e) => errors.push(format!("{}: {e}", item.project)),
        }
    }
    // A forest goes with its last worktree; entries that aren't worktrees were
    // already refused.
    let dir = forest.or_else(|| {
        items
            .first()
            .and_then(|i| i.path.parent().map(Path::to_path_buf))
    });
    if let Some(dir) = dir
        && errors.is_empty()
        && dir.parent() == Some(layout.forests_dir().as_path())
        && fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none())
    {
        match fs::remove_dir(&dir) {
            Ok(()) => text.push_str(&format!("removed {}\n", dir.display())),
            Err(e) => errors.push(format!("cannot remove {}: {e}", dir.display())),
        }
    }
    if !errors.is_empty() && merged.is_empty() {
        return Err(fail!("{}", errors.join("\n")));
    }
    // A partial close still reports what is gone.
    for e in &errors {
        eprintln!("forest: {e}");
    }
    let mut out = Output::new(
        text,
        identity.with(json!({"merged": merged, "errors": errors})),
    );
    if !errors.is_empty() {
        out.code = 1;
    }
    if cwd_in_target {
        out.warnings.push(format!(
            "the current directory was inside {}; cd elsewhere",
            identity.path.display()
        ));
    }
    Ok(out)
}

/// What merging `item` takes, or `None` after piling a refusal onto `refusals`.
fn check(
    layout: &Layout,
    item: &Item,
    delete_branch: bool,
    refusals: &mut Vec<String>,
) -> Result<Option<How>> {
    let p = &item.project;
    let repo = layout.repo_dir(p);
    let Some(branch) = &item.branch else {
        refusals.push(format!("{p} is detached"));
        return Ok(None);
    };
    if !item.missing && is_dirty(&item.path)? {
        refusals.push(format!(
            "{} has uncommitted changes or untracked files",
            item.path.display()
        ));
    }
    // The merge happens in the repo's home, which must hold the default branch
    // and be clean itself.
    let default = default_branch(&repo)?;
    let on = git(&repo, ["branch", "--show-current"]).unwrap_or_default();
    if on.is_empty() {
        refusals.push(format!("home of {p} is not on '{default}'"));
    } else if on != default {
        refusals.push(format!("home of {p} is on '{on}', not '{default}'"));
    }
    if is_dirty(&repo)? {
        refusals.push(format!(
            "home of {p} has uncommitted changes or untracked files"
        ));
    }
    let how = if is_ancestor(&repo, branch, &default) {
        How::AlreadyIn(default)
    } else if is_ancestor(&repo, &default, branch) {
        How::FastForward(default)
    } else {
        refusals.push(format!(
            "{p}: '{branch}' and '{default}' have diverged; merge or rebase first"
        ));
        return Ok(None);
    };
    if delete_branch && *branch == *how.default_branch() {
        refusals.push(format!(
            "'{branch}' is the default branch of {p}; it is never deleted"
        ));
    }
    Ok(Some(how))
}

/// Removes the worktree and, if asked, deletes its branch (`-d`: only when
/// merged, which the merge just made sure of). Returns whether it was deleted.
fn close(layout: &Layout, item: &Item, delete_branch: bool) -> Result<bool> {
    remove(layout, item, false, false)?;
    if delete_branch && let Some(b) = &item.branch {
        git(&layout.repo_dir(&item.project), ["branch", "-d", b])?;
        return Ok(true);
    }
    Ok(false)
}
