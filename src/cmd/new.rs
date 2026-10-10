//! `new`: create a worktree or workforest.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail};
use crate::layout::{Layout, Name, slug};
use crate::project::{
    Worktree, check_branch_name, commit_exists, default_branch, fetch, live_worktrees, origin_url,
    ref_exists, worktree_add,
};

enum Status {
    Created,
    Exists,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Created => "created",
            Status::Exists => "exists",
        }
    }
}

pub fn new(layout: &Layout, target: &str, from: Option<&str>) -> Result<Output> {
    let target = layout.parse_target(target)?;
    match target.name {
        Name::Project(project) => new_worktree(layout, &project, &target.branch, from),
        Name::Forest(forest) => new_workforest(layout, &forest, &target.branch, from),
    }
}

fn new_worktree(
    layout: &Layout,
    project: &str,
    branch: &str,
    from: Option<&str>,
) -> Result<Output> {
    let dir = slug(branch);
    let path = layout.project_dir(project).join(&dir);
    let existing = project_worktrees(layout, project)?;
    // An existing worktree may be addressed by its dir name; keep its real branch.
    let branch = existing_branch(&existing, &path)
        .filter(|b| b == branch || slug(branch) == branch)
        .unwrap_or_else(|| branch.to_string());
    let status = ensure_worktree(layout, project, &existing, &branch, &path, from, false)?;
    eprintln!("{project}: {} {branch}", status.as_str());
    Ok(Output::new(
        format!("{}\n", path.display()),
        Identity {
            kind: "worktree",
            name: project.to_string(),
            dir,
            branch: Some(branch.clone()),
            path: path.clone(),
        }
        .with(json!({
            "worktrees": [{"project": project, "path": path, "status": status.as_str(), "error": null}],
        })),
    ))
}

fn new_workforest(
    layout: &Layout,
    forest: &str,
    branch: &str,
    from: Option<&str>,
) -> Result<Output> {
    let projects = layout.forest_projects(forest)?;
    if projects.is_empty() {
        return Err(fail!("forest '{forest}' has no projects"));
    }
    let dir = layout.forest_dir(forest).join(slug(branch));
    let lists: Vec<Result<Vec<Worktree>>> = projects
        .iter()
        .map(|p| project_worktrees(layout, p))
        .collect();
    // Re-runs fill gaps with the branch the workforest already has, even when it is
    // addressed by its dir name.
    let real = projects
        .iter()
        .zip(&lists)
        .find_map(|(p, list)| existing_branch(list.as_ref().ok()?, &dir.join(p)));
    let branch = match real {
        Some(b) if b == branch || slug(branch) == branch => b,
        Some(b) => {
            return Err(fail!(
                "{} already holds branch '{b}', which collides with '{branch}'",
                dir.display()
            ));
        }
        None => branch.to_string(),
    };
    check_branch_name(&branch)?;

    let created_dir = !dir.exists();
    fs::create_dir_all(&dir)?;
    let mut results = Vec::new();
    let mut failed = 0;
    for (p, list) in projects.iter().zip(lists) {
        let path = dir.join(p);
        let result =
            list.and_then(|list| ensure_worktree(layout, p, &list, &branch, &path, from, true));
        let (status, error) = match &result {
            Ok(s) => {
                eprintln!("{p}: {}", s.as_str());
                (s.as_str(), Value::Null)
            }
            Err(e) => {
                failed += 1;
                eprintln!("{p}: failed: {e}");
                ("failed", json!(e.to_string()))
            }
        };
        results.push(json!({"project": p, "path": path, "status": status, "error": error}));
    }
    if failed == projects.len() && created_dir {
        let _ = fs::remove_dir(&dir);
        return Err(fail!("no worktree could be created for {forest}:{branch}"));
    }
    let mut out = Output::new(
        format!("{}\n", dir.display()),
        Identity::workforest(forest, &dir, Some(branch)).with(json!({"worktrees": results})),
    );
    if failed > 0 {
        eprintln!("forest: {failed} of {} projects failed", projects.len());
        out.code = 1;
    }
    Ok(out)
}

/// The project's worktrees, with registrations whose dir is gone pruned, so a
/// deleted worktree can be recreated.
fn project_worktrees(layout: &Layout, project: &str) -> Result<Vec<Worktree>> {
    if !layout.is_project(project) {
        return Err(fail!("no project named '{project}'"));
    }
    live_worktrees(&layout.bare(project))
}

/// The branch of the worktree registered at `path`, if any.
fn existing_branch(existing: &[Worktree], path: &Path) -> Option<String> {
    existing
        .iter()
        .find(|w| w.path == path)
        .and_then(|w| w.branch.clone())
}

/// Makes `path` a worktree of `project` on `branch`. `existing` is the project's
/// worktree list. With `from_fallback`, a `from` that doesn't exist in this project
/// falls back to the default branch.
fn ensure_worktree(
    layout: &Layout,
    project: &str,
    existing: &[Worktree],
    branch: &str,
    path: &PathBuf,
    from: Option<&str>,
    from_fallback: bool,
) -> Result<Status> {
    check_branch_name(branch)?;
    let bare = layout.bare(project);
    if let Some(w) = existing.iter().find(|w| &w.path == path) {
        return match &w.branch {
            Some(b) if b == branch => Ok(Status::Exists),
            Some(b) => Err(fail!(
                "{} already holds branch '{b}', which collides with '{branch}'",
                path.display()
            )),
            None => Err(fail!("{} holds a detached HEAD", path.display())),
        };
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
    if path.exists() {
        return Err(fail!("{} exists but is not a worktree", path.display()));
    }

    if origin_url(&bare).is_some() {
        fetch(&bare)?;
    }
    let remote = format!("refs/remotes/origin/{branch}");
    if ref_exists(&bare, &format!("refs/heads/{branch}")) {
        worktree_add(&bare, &[], path, &[branch])?;
    } else if ref_exists(&bare, &remote) {
        worktree_add(&bare, &["--track", "-b", branch], path, &[&remote])?;
    } else {
        let base = match from {
            Some(f) if commit_exists(&bare, f) => f.to_string(),
            Some(f) if !from_fallback => return Err(fail!("ref '{f}' not found in {project}")),
            _ => default_base(&bare, project)?,
        };
        worktree_add(&bare, &["--no-track", "-b", branch], path, &[&base])?;
    }
    Ok(Status::Created)
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
