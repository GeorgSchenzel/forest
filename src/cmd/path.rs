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
    /// The forest a worktree belongs to; `None` for one in its project dir, and for
    /// a forest itself.
    pub forest: Option<String>,
}

impl Identity {
    pub fn worktree(layout: &Layout, project: &str, worktree: &Worktree) -> Self {
        Identity {
            kind: "worktree",
            name: project.to_string(),
            dir: worktree.dir_name(),
            branch: worktree.branch.clone(),
            path: worktree.path.clone(),
            forest: layout.forest_of(&worktree.path),
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
            forest: None,
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
            Located::Worktree { project, worktree } => {
                Identity::worktree(layout, project, worktree)
            }
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

/// A target is `<project>:<branch>` (or `:<branch>` in the current project), a
/// worktree wherever it is, or a bare word, a forest. Without one, it is the worktree
/// or forest cwd is in.
pub fn locate(layout: &Layout, target: Option<&str>) -> Result<Located> {
    let Some(t) = target else {
        return locate_cwd(layout);
    };
    if let Some((p, branch)) = layout.split_target(t)? {
        return find_worktree_by_branch(layout, &p, &branch)?
            .ok_or_else(|| fail!("no worktree of {p} on '{branch}'"));
    }
    if let Some(found) = find_forest(layout, t) {
        return Ok(found);
    }
    // A branch of the current project was likely meant.
    if let Some(p) = layout.context_project()
        && find_worktree_by_branch(layout, &p, t)?.is_some()
    {
        return Err(fail!("no forest named '{t}'; for the branch, write ':{t}'"));
    }
    Err(fail!("no forest named '{t}'"))
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
            find_worktree(layout, &p, |list| list.iter().position(|w| w.path == path))?
        }
    }
    .ok_or_else(|| fail!("the current directory is not in a worktree or forest"))
}

/// The worktree on `branch`, in the project dir or a forest (git checks a branch out
/// at most once), else the one in the project dir whose dir name is `branch`.
fn find_worktree_by_branch(
    layout: &Layout,
    project: &str,
    branch: &str,
) -> Result<Option<Located>> {
    let dir = layout.project_dir(project);
    find_worktree(layout, project, |list| {
        list.iter()
            .position(|w| {
                w.branch.as_deref() == Some(branch)
                    && (w.path.parent() == Some(dir.as_path())
                        || layout.forest_of(&w.path).is_some())
            })
            .or_else(|| {
                list.iter()
                    .position(|w| w.path.parent() == Some(dir.as_path()) && w.dir_name() == branch)
            })
    })
}

/// The worktree of `project` that `pick` chooses.
fn find_worktree(
    layout: &Layout,
    project: &str,
    pick: impl Fn(&[Worktree]) -> Option<usize>,
) -> Result<Option<Located>> {
    let mut list = worktrees(&layout.bare(project))?;
    Ok(pick(&list).map(|i| Located::Worktree {
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
