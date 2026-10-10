//! `rm`: remove a worktree or a whole workforest, refusing anything that could lose work.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::error::{Error, Result, fail};
use crate::git::git;
use crate::layout::{Layout, canonical};
use crate::project::{branch_is_safe_to_delete, default_branch, is_dirty, worktrees};

struct Item {
    project: String,
    path: PathBuf,
    branch: Option<String>,
    missing: bool,
}

pub fn rm(
    layout: &Layout,
    target: Option<&str>,
    force: bool,
    delete_branch: bool,
) -> Result<Output> {
    let located = locate(layout, target)?;
    let identity = located.identity(layout)?;
    // Before removing: a deleted cwd can't be resolved.
    let cwd_in_target = env::current_dir().is_ok_and(|c| canonical(&c).starts_with(&identity.path));
    let (items, orphans, workforest) = match located {
        Located::Worktree { project, worktree } => {
            let item = Item {
                missing: !worktree.path.exists(),
                path: worktree.path,
                branch: worktree.branch,
                project,
            };
            (vec![item], vec![], None)
        }
        Located::Workforest { dir, .. } => {
            let (items, orphans) = workforest_items(layout, &dir)?;
            (items, orphans, Some(dir))
        }
    };

    // Check everything first: any refusal refuses the whole rm.
    let mut refusals = Vec::new();
    if !force {
        for item in &items {
            refusals.extend(check(layout, item, delete_branch)?);
        }
        for o in &orphans {
            refusals.push(format!("{} is not a worktree", o.display()));
        }
    }
    if !refusals.is_empty() {
        refusals.push("use --force to override".into());
        return Err(Error::Refused(refusals.join("\n")));
    }

    let mut text = String::new();
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    for item in &items {
        match remove(layout, item, force, delete_branch) {
            Ok(branch_deleted) => {
                text.push_str(&format!("removed {}\n", item.path.display()));
                if branch_deleted && let Some(b) = &item.branch {
                    text.push_str(&format!("deleted branch {b} of {}\n", item.project));
                }
                removed.push(json!({
                    "project": item.project,
                    "branch": item.branch,
                    "path": item.path,
                    "branch_deleted": branch_deleted,
                }));
            }
            Err(e) => errors.push(format!("{}: {e}", item.project)),
        }
    }
    // Even with --force, entries that aren't worktrees are never deleted; they keep
    // the workforest dir.
    if let Some(dir) = &workforest
        && errors.is_empty()
    {
        if !orphans.is_empty() {
            errors.push(format!(
                "kept {}: it holds entries that are not worktrees",
                dir.display()
            ));
        } else {
            match fs::remove_dir(dir) {
                Ok(()) => text.push_str(&format!("removed {}\n", dir.display())),
                Err(e) => errors.push(format!("cannot remove {}: {e}", dir.display())),
            }
        }
    }
    if !errors.is_empty() && removed.is_empty() {
        return Err(fail!("{}", errors.join("\n")));
    }
    // A partial removal still reports what is gone.
    for e in &errors {
        eprintln!("forest: {e}");
    }
    let mut out = Output::new(
        text,
        identity.with(json!({"removed": removed, "errors": errors})),
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

/// The worktrees in a workforest dir, including registered ones whose dir is
/// gone, and entries that aren't worktrees.
fn workforest_items(layout: &Layout, dir: &Path) -> Result<(Vec<Item>, Vec<PathBuf>)> {
    let dir = canonical(dir);
    let mut items = Vec::new();
    // Every project, not just the forest's: a project may have left the forest
    // after its worktree was made.
    for project in layout.projects()? {
        for w in worktrees(&layout.bare(&project))? {
            if w.path.parent() == Some(dir.as_path()) {
                items.push(Item {
                    project: project.clone(),
                    missing: !w.path.exists(),
                    path: w.path,
                    branch: w.branch,
                });
            }
        }
    }
    let mut orphans = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = canonical(&entry?.path());
        if !items.iter().any(|i| i.path == path) {
            orphans.push(path);
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    orphans.sort();
    Ok((items, orphans))
}

fn check(layout: &Layout, item: &Item, delete_branch: bool) -> Result<Vec<String>> {
    let mut refusals = Vec::new();
    let bare = layout.bare(&item.project);
    let default = default_branch(&bare)?;
    let shown = item.path.display();
    if !item.missing && is_dirty(&item.path)? {
        refusals.push(format!(
            "{shown} has uncommitted changes or untracked files"
        ));
    }
    if item.branch.as_deref() == Some(default.as_str()) {
        refusals.push(format!(
            "{shown} is the default-branch worktree of {}",
            item.project
        ));
    }
    if delete_branch
        && let Some(b) = &item.branch
        && !branch_is_safe_to_delete(&bare, b, &default)
    {
        refusals.push(format!(
            "branch '{b}' of {} has commits that are neither on a remote-tracking branch nor on '{default}'",
            item.project
        ));
    }
    Ok(refusals)
}

/// Removes the worktree and, if asked, its branch. Returns whether the branch was deleted.
fn remove(layout: &Layout, item: &Item, force: bool, delete_branch: bool) -> Result<bool> {
    let bare = layout.bare(&item.project);
    if item.missing {
        git(&bare, ["worktree", "prune"])?;
    } else {
        let mut args: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
        if force {
            args.push("--force".as_ref());
        }
        args.push(item.path.as_os_str());
        git(&bare, args)?;
    }
    match (&item.branch, delete_branch) {
        (Some(b), true) => {
            git(&bare, ["branch", "-D", b])?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
