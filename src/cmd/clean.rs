//! `clean`: remove every forest whose work is done.
//!
//! A forest is done when it holds only worktrees, none has uncommitted changes, and
//! each branch is merged into the default branch or has its upstream gone. Branches
//! stay unless asked, and are then deleted only when no commit would be lost.

use std::fs;

use serde_json::json;

use crate::Output;
use crate::cmd::rm::{Item, forest_items, remove};
use crate::error::Result;
use crate::git::git;
use crate::layout::Layout;
use crate::project::{default_branch, is_dirty, is_done, unsaved_commits};

pub fn clean(layout: &Layout, dry_run: bool, delete_branch: bool) -> Result<Output> {
    let verb = if dry_run { "would remove" } else { "removed" };
    let mut text = String::new();
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    let mut failed = false;
    for name in layout.forests()? {
        let dir = layout.forest_dir(&name);
        let (items, orphans) = forest_items(layout, &name, &dir)?;
        let reasons = not_done(layout, &items, !orphans.is_empty())?;
        if !reasons.is_empty() {
            text.push_str(&format!("kept {}: {}\n", dir.display(), reasons.join("; ")));
            kept.push(json!({"forest": name, "path": dir, "reasons": reasons}));
            continue;
        }

        text.push_str(&format!("{verb} {}\n", dir.display()));
        let mut worktrees = Vec::new();
        for item in &items {
            let mut entry =
                json!({"project": item.project, "branch": item.branch, "path": item.path});
            if !dry_run && let Err(e) = remove(layout, item, false, false) {
                text.push_str(&format!("  failed {}: {e}\n", item.path.display()));
                entry["error"] = json!(e.to_string());
                failed = true;
            } else if delete_branch && let Some(b) = &item.branch {
                let (deleted, note) = drop_branch(layout, item, b, dry_run)?;
                text.push_str(&format!("  {note}\n"));
                entry["branch_deleted"] = json!(deleted);
                entry["branch_note"] = json!(note);
            }
            worktrees.push(entry);
        }
        if !dry_run && fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none()) {
            fs::remove_dir(&dir)?;
        }
        removed.push(json!({"forest": name, "path": dir, "worktrees": worktrees}));
    }
    if removed.is_empty() && kept.is_empty() {
        text.push_str("no forests\n");
    }
    let mut out = Output::new(
        text,
        json!({"dry_run": dry_run, "removed": removed, "kept": kept}),
    );
    if failed {
        out.code = 1;
    }
    Ok(out)
}

/// Why a forest isn't done; empty when it is.
fn not_done(layout: &Layout, items: &[Item], strays: bool) -> Result<Vec<String>> {
    let mut reasons = Vec::new();
    if items.is_empty() {
        reasons.push("no worktrees (see prune)".to_string());
    }
    if strays {
        reasons.push("holds entries that are not worktrees".to_string());
    }
    for item in items {
        let p = &item.project;
        if !item.missing && is_dirty(&item.path)? {
            reasons.push(format!("{p} has uncommitted changes"));
        }
        let repo = layout.repo_dir(p);
        match &item.branch {
            None => reasons.push(format!("{p} is detached")),
            Some(b) if !is_done(&repo, b, &default_branch(&repo)?) => {
                reasons.push(format!(
                    "{p}: '{b}' is not merged and its upstream is not gone"
                ));
            }
            Some(_) => {}
        }
    }
    Ok(reasons)
}

/// Deletes the branch if no commit would be lost. Returns whether it was (or would
/// be) deleted, and what happened.
fn drop_branch(
    layout: &Layout,
    item: &Item,
    branch: &str,
    dry_run: bool,
) -> Result<(bool, String)> {
    let repo = layout.repo_dir(&item.project);
    let default = default_branch(&repo)?;
    let p = &item.project;
    match unsaved_commits(&repo, branch, &default) {
        Some(0) if branch != default => {
            if !dry_run {
                git(&repo, ["branch", "-D", branch])?;
            }
            let verb = if dry_run { "would delete" } else { "deleted" };
            Ok((true, format!("{verb} branch {branch} of {p}")))
        }
        Some(n) if n > 0 => Ok((
            false,
            format!(
                "kept branch {branch} of {p}: {n} commits are on no remote and not on '{default}'"
            ),
        )),
        _ => Ok((false, format!("kept branch {branch} of {p}"))),
    }
}
