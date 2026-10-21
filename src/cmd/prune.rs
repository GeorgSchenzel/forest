//! `prune`: `git worktree prune` in every repo, and report forest dirs that aren't worktrees.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::error::Result;
use crate::git::git_stderr;
use crate::layout::Layout;
use crate::project::worktrees;

pub fn prune(layout: &Layout) -> Result<Output> {
    let mut text = String::new();
    let mut pruned = Vec::new();
    let mut registered = HashSet::new();
    for p in &layout.projects()? {
        let repo = layout.repo_dir(p);
        let out = git_stderr(&repo, ["worktree", "prune", "--verbose"])?;
        for line in out.lines().filter(|l| !l.is_empty()) {
            text.push_str(&format!("{p}: {line}\n"));
            pruned.push(json!({"project": p, "message": line}));
        }
        registered.extend(worktrees(&repo)?.into_iter().map(|w| w.path));
    }

    let mut orphans = Vec::new();
    for f in layout.forests()? {
        let forest = layout.forest_dir(&f);
        let members = subdirs(&forest)?;
        // No worktrees left: the dir is a leftover, whatever files it still holds.
        if members.is_empty() {
            orphans.push(forest);
            continue;
        }
        orphans.extend(members.into_iter().filter(|m| !registered.contains(m)));
    }
    for o in &orphans {
        text.push_str(&format!("orphaned: {}\n", o.display()));
    }
    if pruned.is_empty() && orphans.is_empty() {
        text.push_str("nothing to prune\n");
    }
    Ok(Output::new(
        text,
        json!({"pruned": pruned, "orphans": orphans}),
    ))
}

fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path().canonicalize()?);
        }
    }
    dirs.sort();
    Ok(dirs)
}
