//! `path`, and resolving a target (or cwd) to a worktree or forest.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::Output;
use crate::error::{Result, fail};
use crate::layout::{Layout, Name};
use crate::project::{Worktree, worktrees};

pub enum Located {
    Worktree { project: String, worktree: Worktree },
    Forest { forest: String, dir: PathBuf },
}

/// What a worktree or forest is, the same however it was addressed. In JSON
/// output, next to the command's own fields.
#[derive(Serialize)]
pub struct Identity {
    /// `"worktree"` or `"forest"`.
    pub kind: &'static str,
    /// The project of a worktree, or the forest's own name.
    pub name: String,
    pub dir: String,
    /// `None` when detached, or for a forest without a worktree on a branch.
    pub branch: Option<String>,
    pub path: PathBuf,
}

impl Identity {
    pub fn worktree(project: &str, worktree: &Worktree) -> Self {
        Identity {
            kind: "worktree",
            name: project.to_string(),
            dir: worktree.dir_name(),
            branch: worktree.branch.clone(),
            path: worktree.path.clone(),
        }
    }

    /// `branch` as `ls` reports it: that of the first member worktree on one.
    pub fn forest(forest: &str, dir: &Path, branch: Option<String>) -> Self {
        Identity {
            kind: "forest",
            name: forest.to_string(),
            dir: forest.to_string(),
            branch,
            path: dir.to_path_buf(),
        }
    }

    /// The identity's fields, then `extra`'s.
    pub fn with(&self, extra: Value) -> Value {
        let mut out = json!(self);
        if let (Value::Object(out), Value::Object(extra)) = (&mut out, extra) {
            out.extend(extra);
        }
        out
    }
}

impl Located {
    pub fn path(&self) -> &PathBuf {
        match self {
            Located::Worktree { worktree, .. } => &worktree.path,
            Located::Forest { dir, .. } => dir,
        }
    }

    pub fn identity(&self, layout: &Layout) -> Result<Identity> {
        Ok(match self {
            Located::Worktree { project, worktree } => Identity::worktree(project, worktree),
            Located::Forest { forest, dir } => {
                Identity::forest(forest, dir, forest_branch(layout, dir)?)
            }
        })
    }
}

pub fn path(layout: &Layout, target: Option<&str>) -> Result<Output> {
    let located = locate(layout, target)?;
    let path = located.path();
    Ok(Output::new(
        format!("{}\n", path.display()),
        located.identity(layout)?.with(json!({})),
    ))
}

/// The branch of the first member worktree on one, in project order.
fn forest_branch(layout: &Layout, dir: &Path) -> Result<Option<String>> {
    for project in layout.projects()? {
        if let Some(w) = worktrees(&layout.bare(&project))?
            .into_iter()
            .find(|w| w.path.parent() == Some(dir))
            && w.branch.is_some()
        {
            return Ok(w.branch);
        }
    }
    Ok(None)
}

/// A target is `<project>:<branch>`, or a bare word: a worktree of the project cwd
/// is in, else a forest. Without one, it is the worktree or forest cwd is in.
pub fn locate(layout: &Layout, target: Option<&str>) -> Result<Located> {
    let Some(t) = target else {
        return locate_cwd(layout);
    };
    let found = match layout.split_target(t)? {
        Some((p, branch)) => find_worktree_by_branch(layout, &p, &branch)?,
        None => {
            let in_project = match layout.context_project() {
                Some(p) => find_worktree_by_branch(layout, &p, t)?,
                None => None,
            };
            match in_project {
                Some(found) => Some(found),
                None => find_forest(layout, t),
            }
        }
    };
    found.ok_or_else(|| fail!("no worktree or forest found for '{t}'"))
}

fn locate_cwd(layout: &Layout) -> Result<Located> {
    let ctx = layout
        .context()
        .ok_or_else(|| fail!("not inside a project or forest; give a target"))?;
    match ctx.name {
        Name::Forest(f) => find_forest(layout, &f),
        Name::Project(p) => {
            let Some(sub) = ctx.sub else {
                return Err(fail!("not inside a worktree of '{p}'; give a branch"));
            };
            let path = layout.project_dir(&p).join(&sub);
            find_worktree(layout, &p, &[&|w| w.path == path])?
        }
    }
    .ok_or_else(|| fail!("the current directory is not in a worktree or forest"))
}

/// The real branch name wins over a dir name that happens to equal it.
fn find_worktree_by_branch(
    layout: &Layout,
    project: &str,
    branch: &str,
) -> Result<Option<Located>> {
    find_worktree(
        layout,
        project,
        &[&|w| w.branch.as_deref() == Some(branch), &|w| {
            w.dir_name() == branch
        }],
    )
}

/// The first worktree in the project dir matching the first `preds` entry that
/// matches any.
fn find_worktree(
    layout: &Layout,
    project: &str,
    preds: &[&dyn Fn(&Worktree) -> bool],
) -> Result<Option<Located>> {
    let dir = layout.project_dir(project);
    let mut list: Vec<Worktree> = worktrees(&layout.bare(project))?
        .into_iter()
        .filter(|w| w.path.parent() == Some(dir.as_path()))
        .collect();
    Ok(preds
        .iter()
        .find_map(|pred| list.iter().position(pred))
        .map(|i| Located::Worktree {
            project: project.to_string(),
            worktree: list.swap_remove(i),
        }))
}

fn find_forest(layout: &Layout, forest: &str) -> Option<Located> {
    layout.is_forest(forest).then(|| Located::Forest {
        forest: forest.to_string(),
        dir: layout.forest_dir(forest),
    })
}
