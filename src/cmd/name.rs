//! `name`: give the work of the forest you are in its name, on its branch.

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail, refused};
use crate::git::{WorktreeGit, git_in_worktree, git_in_worktree_out};
use crate::layout::{Context, Layout, slug};
use crate::project::{
    all_worktrees, check_branch_name, forests_on, members, one_branch, ref_exists,
};

/// Renames the branch of every worktree in the forest of the cwd. Everything is
/// checked first, so a forest is never half-renamed. The forest's dir keeps its
/// name — renaming it would take its box and agents down — and the output says
/// how to rename it later.
pub fn name(layout: &Layout, new: &str) -> Result<Output> {
    let Some(Context::Forest(forest)) = layout.context() else {
        return Err(refused!(
            "not inside a forest; `forest name` names the work of the forest you are in"
        ));
    };
    let members = members(layout, &forest)?;
    let old = one_branch(&forest, &members)?;
    check_branch_name(new).map_err(|_| refused!("invalid branch name '{new}'"))?;
    if new == old {
        return Err(refused!("forest '{forest}' is on '{old}' already"));
    }
    // A target resolves by forest name, forest branch or repo name; a branch
    // that answers to any of those would be ambiguous.
    if layout.is_forest(new) && new != forest {
        return Err(refused!("a forest is named '{new}' already"));
    }
    if layout.is_project(new) {
        return Err(refused!("a repo is named '{new}' already"));
    }
    let all = all_worktrees(layout)?;
    if let Some(f) = forests_on(&all, layout, new)
        .iter()
        .find(|f| f.as_str() != forest)
    {
        return Err(refused!("'{new}' is the branch of forest '{f}' already"));
    }
    for (project, _) in &members {
        if ref_exists(&layout.repo_dir(project), &format!("refs/heads/{new}")) {
            return Err(refused!("branch '{new}' exists in {project} already"));
        }
    }

    // B3: every member's git dirs must match the layout before anything is
    // touched — `branch -m` runs in box-writable worktrees, and a rewritten
    // `.git` pointer must not choose the config (or code) it runs with.
    let pinned: Vec<(String, WorktreeGit)> = members
        .iter()
        .map(|(project, w)| {
            WorktreeGit::new(&layout.repo_dir(project), &w.path)
                .map(|dirs| (project.clone(), dirs))
        })
        .collect::<Result<_>>()
        .map_err(|e| refused!("{e}"))?;

    let mut warnings = Vec::new();
    let mut done = Vec::new();
    let mut errors = Vec::new();
    for (project, dirs) in &pinned {
        // git moves the upstream config to the renamed branch; the remote
        // branch itself keeps its old name, which is worth saying.
        if git_in_worktree_out(dirs, ["config", &format!("branch.{old}.remote")]).is_some() {
            warnings.push(format!(
                "{project}: '{new}' still tracks '{old}'s upstream; the remote branch keeps its old name"
            ));
        }
        match git_in_worktree(dirs, ["branch", "-m", &old, new]) {
            Ok(_) => done.push(project.as_str()),
            Err(e) => errors.push(format!("{project}: {e}")),
        }
    }
    if !errors.is_empty() {
        let done = if done.is_empty() {
            "none were renamed".to_string()
        } else {
            format!("renamed {old} to {new} in {}", done.join(", "))
        };
        return Err(fail!("{done}; failed:\n{}", errors.join("\n")));
    }

    let dir = layout.forest_dir(&forest);
    let names: Vec<&str> = members.iter().map(|(p, _)| p.as_str()).collect();
    let mut text = format!("renamed {old} to {new} in {}\n", names.join(", "));
    let folder = slug(new);
    if folder != forest {
        text.push_str(&format!(
            "the folder keeps its name; rename it later with `forest mv {forest} {folder}` and `bento start` when no agents run\n"
        ));
    }
    let worktrees: Vec<Value> = members
        .iter()
        .map(|(p, w)| json!({"project": p, "path": w.path}))
        .collect();
    let mut out = Output::new(
        text,
        Identity::forest(&forest, &dir, Some(new.to_string()))
            .with(json!({"old": old, "new": new, "worktrees": worktrees})),
    );
    out.warnings = warnings;
    Ok(out)
}
