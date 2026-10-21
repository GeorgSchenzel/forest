//! `new`: create a worktree, or a forest with one worktree per project.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail};
use crate::git::git;
use crate::layout::{Layout, slug, valid_forest_name};
use crate::project::{
    check_branch_name, commit_exists, default_branch, fetch, live_worktrees, origin_url,
    ref_exists, worktree_add,
};

pub fn new(
    layout: &Layout,
    target: &str,
    projects: &[String],
    branch: Option<&str>,
    from: Option<&str>,
) -> Result<Output> {
    if !projects.is_empty() {
        return new_forest(layout, target, projects, branch.unwrap_or(target), from);
    }
    if branch.is_some() {
        return Err(fail!(
            "--branch is for forests: forest new <name> <project>... --branch <branch>"
        ));
    }
    let (project, branch) = match layout.split_target(target)? {
        Some(t) => t,
        None => match layout.context_project() {
            Some(p) => (p, target.to_string()),
            None => {
                return Err(fail!(
                    "'{target}' needs projects to be a forest, or <project>:<branch> to be a worktree"
                ));
            }
        },
    };
    new_worktree(layout, &project, &branch, from)
}

fn new_worktree(
    layout: &Layout,
    project: &str,
    branch: &str,
    from: Option<&str>,
) -> Result<Output> {
    check_project(layout, project)?;
    let dir = slug(branch);
    let path = layout.project_dir(project).join(&dir);
    create_worktree(layout, project, branch, &path, from, false)?;
    eprintln!("{project}: created {branch}");
    Ok(Output::new(
        format!("{}\n", path.display()),
        Identity {
            kind: "worktree",
            name: project.to_string(),
            dir,
            branch: Some(branch.to_string()),
            path: path.clone(),
        }
        .with(json!({"worktrees": [{"project": project, "path": path}]})),
    ))
}

/// All or nothing: if any project fails, the worktrees and branches made so far are
/// removed again, and so is the forest dir.
fn new_forest(
    layout: &Layout,
    name: &str,
    projects: &[String],
    branch: &str,
    from: Option<&str>,
) -> Result<Output> {
    valid_forest_name(name)?;
    if layout.is_project(name) {
        return Err(fail!(
            "a project named '{name}' exists; a forest can't share its name"
        ));
    }
    let dir = layout.forest_dir(name);
    if dir.exists() {
        return Err(fail!("forest '{name}' already exists at {}", dir.display()));
    }
    check_branch_name(branch)?;
    let mut unique: Vec<&String> = Vec::new();
    for p in projects {
        check_project(layout, p)?;
        if !unique.contains(&p) {
            unique.push(p);
        }
    }

    fs::create_dir_all(&dir)?;
    let mut made: Vec<(&str, std::path::PathBuf, bool)> = Vec::new();
    for p in &unique {
        let path = dir.join(p);
        match create_worktree(layout, p, branch, &path, from, true) {
            Ok(branch_created) => {
                eprintln!("{p}: created {branch}");
                made.push((p, path, branch_created));
            }
            Err(e) => {
                for (p, path, branch_created) in made.iter().rev() {
                    let bare = layout.bare(p);
                    let _ = git(
                        &bare,
                        [
                            "worktree".as_ref(),
                            "remove".as_ref(),
                            "--force".as_ref(),
                            path.as_os_str(),
                        ],
                    );
                    if *branch_created {
                        let _ = git(&bare, ["branch", "-D", branch]);
                    }
                }
                let _ = fs::remove_dir_all(&dir);
                return Err(fail!("{p}: {e}\nnothing was created"));
            }
        }
    }
    let worktrees: Vec<_> = made
        .iter()
        .map(|(p, path, _)| json!({"project": p, "path": path}))
        .collect();
    Ok(Output::new(
        format!("{}\n", dir.display()),
        Identity::forest(name, &dir, Some(branch.to_string()))
            .with(json!({"worktrees": worktrees})),
    ))
}

fn check_project(layout: &Layout, project: &str) -> Result<()> {
    if layout.is_project(project) {
        Ok(())
    } else {
        Err(fail!("no project named '{project}'"))
    }
}

/// Makes `path` a new worktree of `project` on `branch`; returns whether the branch
/// was created. With `from_fallback`, a `from` that doesn't exist in this project
/// falls back to the default branch.
fn create_worktree(
    layout: &Layout,
    project: &str,
    branch: &str,
    path: &Path,
    from: Option<&str>,
    from_fallback: bool,
) -> Result<bool> {
    check_branch_name(branch)?;
    let bare = layout.bare(project);
    // Pruned first, so a deleted worktree can be made again.
    let existing = live_worktrees(&bare)?;
    if path.exists() || existing.iter().any(|w| w.path == path) {
        return Err(fail!("{} already exists", path.display()));
    }
    if let Some(w) = existing
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        return Err(fail!(
            "branch '{branch}' is already checked out at {}",
            w.path.display()
        ));
    }

    if origin_url(&bare).is_some() {
        fetch(&bare)?;
    }
    let remote = format!("refs/remotes/origin/{branch}");
    if ref_exists(&bare, &format!("refs/heads/{branch}")) {
        worktree_add(&bare, &[], path, &[branch])?;
        return Ok(false);
    }
    if ref_exists(&bare, &remote) {
        worktree_add(&bare, &["--track", "-b", branch], path, &[&remote])?;
    } else {
        let base = match from {
            Some(f) if commit_exists(&bare, f) => f.to_string(),
            Some(f) if !from_fallback => return Err(fail!("ref '{f}' not found in {project}")),
            _ => default_base(&bare, project)?,
        };
        worktree_add(&bare, &["--no-track", "-b", branch], path, &[&base])?;
    }
    Ok(true)
}

/// The default branch to start new branches from: `origin/<default>` when it exists
/// (just fetched, so freshest), else the local one.
fn default_base(bare: &Path, project: &str) -> Result<String> {
    let default = default_branch(bare)?;
    for r in [
        format!("refs/remotes/origin/{default}"),
        format!("refs/heads/{default}"),
    ] {
        if commit_exists(bare, &r) {
            return Ok(r);
        }
    }
    Err(fail!(
        "the default branch '{default}' of {project} has no commits yet; make a first commit"
    ))
}
