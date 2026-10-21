//! `new`: put worktrees on a branch into its forest, making the forest if needed.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail};
use crate::git::git;
use crate::layout::{Layout, slug, valid_name};
use crate::project::{
    Worktree, all_worktrees, check_branch_name, commit_exists, default_branch, fetch, forests_on,
    live_worktrees, origin_url, ref_exists, worktree_add,
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

/// Puts a worktree of each project on `branch` into the forest that already has
/// that branch, else into a new forest named `alias` or after the branch.
pub fn new(
    layout: &Layout,
    branch: &str,
    projects: &[String],
    alias: Option<&str>,
    from: Option<&str>,
) -> Result<Output> {
    check_branch_name(branch)?;
    let mut unique: Vec<&String> = Vec::new();
    for p in projects {
        check_project(layout, p)?;
        if !unique.contains(&p) {
            unique.push(p);
        }
    }

    let all = all_worktrees(layout)?;
    let holding = forests_on(&all, layout, branch);
    let (name, created) = match holding.as_slice() {
        [f] => {
            if let Some(a) = alias.filter(|a| a != f) {
                return Err(fail!(
                    "'{branch}' is in forest '{f}' already; --as '{a}' only names a new forest"
                ));
            }
            (f.clone(), false)
        }
        [] => {
            let name = alias.map_or_else(|| slug(branch), str::to_string);
            check_new_name(layout, &all, &name, None)?;
            (name, true)
        }
        _ => {
            return Err(fail!(
                "'{branch}' is in several forests: {}",
                holding.join(", ")
            ));
        }
    };
    let dir = layout.forest_dir(&name);
    for p in &unique {
        if dir.join(p).exists() {
            return Err(fail!("{} already exists", dir.join(p).display()));
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
                    let repo = layout.repo_dir(p);
                    let _ = git(
                        &repo,
                        [
                            "worktree".as_ref(),
                            "remove".as_ref(),
                            "--force".as_ref(),
                            path.as_os_str(),
                        ],
                    );
                    if start.created() {
                        let _ = git(&repo, ["branch", "-D", branch]);
                    }
                }
                if created {
                    let _ = fs::remove_dir_all(&dir);
                }
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
        Identity::forest(&name, &dir, Some(branch.to_string()))
            .with(json!({"created": created, "worktrees": worktrees})),
    ))
}

/// A new forest's name must be free, and must not name anything else a target can;
/// `own` is the forest being renamed, whose branch it may take.
pub fn check_new_name(
    layout: &Layout,
    all: &[(String, Worktree)],
    name: &str,
    own: Option<&str>,
) -> Result<()> {
    valid_name(name)?;
    let dir = layout.forest_dir(name);
    if dir.exists() {
        return Err(fail!(
            "{} already exists; pick another name with --as",
            dir.display()
        ));
    }
    if layout.is_project(name) {
        return Err(fail!(
            "'{name}' is the name of a repo; pick another name with --as"
        ));
    }
    if let Some(f) = forests_on(all, layout, name)
        .iter()
        .find(|f| Some(f.as_str()) != own)
    {
        return Err(fail!(
            "'{name}' is the branch of forest '{f}'; pick another name with --as"
        ));
    }
    Ok(())
}

fn check_project(layout: &Layout, project: &str) -> Result<()> {
    if layout.is_project(project) {
        Ok(())
    } else {
        Err(fail!("no repo named '{project}'"))
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
    let repo = layout.repo_dir(project);
    // Pruned first, so a deleted worktree can be made again.
    let existing = live_worktrees(&repo)?;
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

    if origin_url(&repo).is_some() {
        fetch(&repo)?;
    }
    let remote = format!("origin/{branch}");
    let exists = if ref_exists(&repo, &format!("refs/heads/{branch}")) {
        Some(Start::Local)
    } else if ref_exists(&repo, &format!("refs/remotes/{remote}")) {
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
                worktree_add(&repo, &["--track", "-b", branch], path, &[&r])?
            }
            _ => worktree_add(&repo, &[], path, &[branch])?,
        }
        return Ok(start);
    }
    let base = match from {
        Some(f) if commit_exists(&repo, f) => f.to_string(),
        Some(f) => return Err(fail!("ref '{f}' not found in {project}")),
        None => default_base(&repo, project)?,
    };
    worktree_add(&repo, &["--no-track", "-b", branch], path, &[&base])?;
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
fn default_base(repo: &Path, project: &str) -> Result<String> {
    let default = default_branch(repo)?;
    for r in [
        format!("refs/remotes/origin/{default}"),
        format!("refs/heads/{default}"),
    ] {
        if commit_exists(repo, &r) {
            return Ok(r);
        }
    }
    Err(fail!(
        "the default branch '{default}' of {project} has no commits yet; make a first commit"
    ))
}
