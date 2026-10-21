//! `new`: create a worktree, or a forest with one worktree per project.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail};
use crate::git::git;
use crate::layout::{Layout, slug, valid_forest_name};
use crate::project::{
    check_branch_name, commit_exists, default_branch, fetch, live_worktrees, origin_url,
    ref_exists, worktree_add,
};

/// Where a new worktree's branch came from.
enum Start {
    /// An existing local branch.
    Local,
    /// Created to track this remote branch.
    Remote(String),
    /// Created from this ref.
    New(String),
}

impl Start {
    fn created(&self) -> bool {
        !matches!(self, Start::Local)
    }

    fn from(&self) -> Option<&str> {
        match self {
            Start::Local => None,
            Start::Remote(r) | Start::New(r) => Some(r),
        }
    }

    fn report(&self, project: &str, branch: &str) {
        match self {
            Start::Local => eprintln!("{project}: checked out branch {branch}"),
            Start::Remote(r) => eprintln!("{project}: created branch {branch} tracking {r}"),
            Start::New(r) => eprintln!("{project}: created branch {branch} from {r}"),
        }
    }

    fn json(&self, project: &str, path: &Path) -> Value {
        json!({
            "project": project,
            "path": path,
            "branch_created": self.created(),
            "from": self.from(),
        })
    }
}

/// With `projects`, `target` is the forest's branch; else it is `<project>:<branch>`
/// or `:<branch>`.
pub fn new(
    layout: &Layout,
    target: &str,
    projects: &[String],
    name: Option<&str>,
    from: Option<&str>,
) -> Result<Output> {
    if !projects.is_empty() {
        check_branch_name(target)?;
        let name = name.map_or_else(|| slug(target), str::to_string);
        return new_forest(layout, &name, projects, target, from);
    }
    if name.is_some() {
        return Err(fail!(
            "--name is for forests: forest new <branch> <project>... --name <forest>"
        ));
    }
    let Some((project, branch)) = layout.split_target(target)? else {
        return Err(fail!(
            "'{target}' needs projects to make a forest (forest new {target} <project>...), \
             or <project>:{target} or :{target} to make a worktree"
        ));
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
    let start = create_worktree(layout, project, branch, &path, from)?;
    start.report(project, branch);
    Ok(Output::new(
        format!("{}\n", path.display()),
        Identity {
            kind: "worktree",
            name: project.to_string(),
            dir,
            branch: Some(branch.to_string()),
            path: path.clone(),
            forest: None,
        }
        .with(json!({"worktrees": [start.json(project, &path)]})),
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
            "a project named '{name}' exists; a forest can't share its name (use --name)"
        ));
    }
    let dir = layout.forest_dir(name);
    if dir.exists() {
        return Err(fail!("forest '{name}' already exists at {}", dir.display()));
    }
    let mut unique: Vec<&String> = Vec::new();
    for p in projects {
        check_project(layout, p)?;
        if !unique.contains(&p) {
            unique.push(p);
        }
    }

    fs::create_dir_all(&dir)?;
    let mut made: Vec<(&str, PathBuf, Start)> = Vec::new();
    for p in &unique {
        let path = dir.join(p);
        match create_worktree(layout, p, branch, &path, from) {
            Ok(start) => {
                start.report(p, branch);
                made.push((p, path, start));
            }
            Err(e) => {
                for (p, path, start) in made.iter().rev() {
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
                    if start.created() {
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
        .map(|(p, path, start)| start.json(p, path))
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

/// Makes `path` a new worktree of `project` on `branch`. `from` is only for a new
/// branch, and must exist.
fn create_worktree(
    layout: &Layout,
    project: &str,
    branch: &str,
    path: &Path,
    from: Option<&str>,
) -> Result<Start> {
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
    let remote = format!("origin/{branch}");
    let exists = if ref_exists(&bare, &format!("refs/heads/{branch}")) {
        Some(Start::Local)
    } else if ref_exists(&bare, &format!("refs/remotes/{remote}")) {
        Some(Start::Remote(remote))
    } else {
        None
    };
    if let Some(start) = exists {
        if from.is_some() {
            let place = match start {
                Start::Remote(_) => "on origin",
                _ => "locally",
            };
            return Err(fail!(
                "branch '{branch}' already exists {place}; --from is only for a new branch"
            ));
        }
        match &start {
            Start::Remote(r) => {
                let r = format!("refs/remotes/{r}");
                worktree_add(&bare, &["--track", "-b", branch], path, &[&r])?
            }
            _ => worktree_add(&bare, &[], path, &[branch])?,
        }
        return Ok(start);
    }
    let base = match from {
        Some(f) if commit_exists(&bare, f) => f.to_string(),
        Some(f) => return Err(fail!("ref '{f}' not found in {project}")),
        None => default_base(&bare, project)?,
    };
    worktree_add(&bare, &["--no-track", "-b", branch], path, &[&base])?;
    Ok(Start::New(short_ref(&base).to_string()))
}

/// `origin/main` for `refs/remotes/origin/main`, `main` for `refs/heads/main`.
fn short_ref(r: &str) -> &str {
    r.strip_prefix("refs/remotes/")
        .or_else(|| r.strip_prefix("refs/heads/"))
        .unwrap_or(r)
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
