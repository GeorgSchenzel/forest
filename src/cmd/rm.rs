//! `rm`: remove a forest or one of its worktrees, refusing anything that could lose work.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::error::{Error, Result, fail};
use crate::git::git;
use crate::layout::{Layout, canonical};
use crate::project::{default_branch, is_dirty, members, unsaved_commits};

pub struct Item {
    pub project: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub missing: bool,
}

pub fn rm(
    layout: &Layout,
    target: Option<&str>,
    project: Option<&str>,
    force: bool,
    delete_branch: bool,
) -> Result<Output> {
    let located = locate(layout, target, project)?;
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
                    "{} is the home of {project}; forest never removes it",
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

    // Check everything first: any refusal refuses the whole rm.
    let mut refusals = Vec::new();
    for item in &items {
        refusals.extend(check(layout, item, force, delete_branch)?);
    }
    if !force {
        for o in &orphans {
            refusals.push(format!("{} is not a worktree", o.display()));
        }
    }
    if !refusals.is_empty() {
        if !force {
            refusals.push("use --force to override".into());
        }
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
    // A forest goes with its last worktree. Entries that aren't worktrees are never
    // deleted, even with --force; they keep the forest dir.
    let dir = forest.or_else(|| {
        items
            .first()
            .and_then(|i| i.path.parent().map(Path::to_path_buf))
    });
    if let Some(dir) = dir
        && errors.is_empty()
        && dir.parent() == Some(layout.forests_dir().as_path())
    {
        if fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none()) {
            match fs::remove_dir(&dir) {
                Ok(()) => text.push_str(&format!("removed {}\n", dir.display())),
                Err(e) => errors.push(format!("cannot remove {}: {e}", dir.display())),
            }
        } else if !orphans.is_empty() {
            errors.push(format!(
                "kept {}: it holds entries that are not worktrees",
                dir.display()
            ));
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

/// The worktrees in a forest dir, including registered ones whose dir is gone, and
/// entries that aren't worktrees.
pub fn forest_items(layout: &Layout, name: &str, dir: &Path) -> Result<(Vec<Item>, Vec<PathBuf>)> {
    let items: Vec<Item> = members(layout, name)?
        .into_iter()
        .map(|(project, w)| Item {
            project,
            missing: !w.path.exists(),
            path: w.path,
            branch: w.branch,
        })
        .collect();
    let mut orphans = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = canonical(&entry?.path());
        if !items.iter().any(|i| i.path == path) {
            orphans.push(path);
        }
    }
    orphans.sort();
    Ok((items, orphans))
}

/// Why removing `item` could lose work. With `force`, only a default branch still
/// refuses deletion.
pub fn check(
    layout: &Layout,
    item: &Item,
    force: bool,
    delete_branch: bool,
) -> Result<Vec<String>> {
    let mut refusals = Vec::new();
    let repo = layout.repo_dir(&item.project);
    let shown = item.path.display();
    if !force && !item.missing && is_dirty(&item.path)? {
        refusals.push(format!(
            "{shown} has uncommitted changes or untracked files"
        ));
    }
    if delete_branch && let Some(b) = &item.branch {
        let default = default_branch(&repo)?;
        if *b == default {
            refusals.push(format!(
                "'{b}' is the default branch of {}; it is never deleted",
                item.project
            ));
        } else if !force && unsaved_commits(&repo, b, &default) != Some(0) {
            refusals.push(format!(
                "branch '{b}' of {} has commits that are neither on a remote-tracking branch nor on '{default}'",
                item.project
            ));
        }
    }
    Ok(refusals)
}

/// Removes the worktree and, if asked, its branch. Returns whether the branch was deleted.
pub fn remove(layout: &Layout, item: &Item, force: bool, delete_branch: bool) -> Result<bool> {
    let repo = layout.repo_dir(&item.project);
    if item.missing {
        git(&repo, ["worktree", "prune"])?;
    } else {
        let mut args: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
        if force {
            args.push("--force".as_ref());
        }
        args.push(item.path.as_os_str());
        git(&repo, args)?;
    }
    match (&item.branch, delete_branch) {
        (Some(b), true) => {
            git(&repo, ["branch", "-D", b])?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
