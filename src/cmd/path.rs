//! `path`, and resolving a target (or cwd) to a worktree or workforest.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::Output;
use crate::error::{Result, fail};
use crate::layout::{Layout, Name, slug};
use crate::project::{Worktree, worktrees};

pub enum Located {
    Worktree { project: String, worktree: Worktree },
    Workforest { forest: String, dir: PathBuf },
}

/// What a worktree or workforest is, the same however it was addressed. In JSON
/// output, next to the command's own fields.
#[derive(Serialize)]
pub struct Identity {
    /// `"worktree"` or `"workforest"`.
    pub kind: &'static str,
    /// The project or forest: the `<name>` in `<name>:<branch>`.
    pub name: String,
    pub dir: String,
    /// `None` when detached, or for a workforest without a worktree on a branch.
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
    pub fn workforest(forest: &str, dir: &Path, branch: Option<String>) -> Self {
        Identity {
            kind: "workforest",
            name: forest.to_string(),
            dir: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
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
            Located::Workforest { dir, .. } => dir,
        }
    }

    pub fn identity(&self, layout: &Layout) -> Result<Identity> {
        Ok(match self {
            Located::Worktree { project, worktree } => Identity::worktree(project, worktree),
            Located::Workforest { forest, dir } => {
                Identity::workforest(forest, dir, workforest_branch(layout, forest, dir)?)
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

/// The branch of the first of the forest's projects with a worktree in `dir`.
fn workforest_branch(layout: &Layout, forest: &str, dir: &Path) -> Result<Option<String>> {
    for project in layout.forest_projects(forest)? {
        if !layout.is_project(&project) {
            continue;
        }
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

/// A target names a project worktree (in the project dir) or a workforest; without
/// one, it is the worktree or workforest cwd is in.
pub fn locate(layout: &Layout, target: Option<&str>) -> Result<Located> {
    let (name, found) = match target {
        Some(t) => {
            let t = layout.parse_target(t)?;
            let found = match &t.name {
                // The real branch name wins over a dir name that happens to equal it.
                Name::Project(p) => find_worktree(
                    layout,
                    p,
                    &[&|w| w.branch.as_deref() == Some(t.branch.as_str()), &|w| {
                        w.dir_name() == t.branch
                    }],
                )?,
                Name::Forest(f) => find_workforest(layout, f, &slug(&t.branch)),
            };
            let what = format!("{}:{}", name_str(&t.name), t.branch);
            (what, found)
        }
        None => {
            let ctx = layout.context().ok_or_else(|| {
                fail!("not inside a project or forest; give a target <name>:<branch>")
            })?;
            let Some(sub) = ctx.sub else {
                return Err(fail!(
                    "not inside a worktree or workforest of '{}'; give a branch",
                    name_str(&ctx.name)
                ));
            };
            let found = match &ctx.name {
                Name::Project(p) => {
                    let path = layout.project_dir(p).join(&sub);
                    find_worktree(layout, p, &[&|w| w.path == path])?
                }
                Name::Forest(f) => find_workforest(layout, f, &sub),
            };
            (format!("the current directory ({sub})"), found)
        }
    };
    found.ok_or_else(|| fail!("no worktree or workforest found for {name}"))
}

fn name_str(name: &Name) -> &str {
    match name {
        Name::Project(n) | Name::Forest(n) => n,
    }
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

fn find_workforest(layout: &Layout, forest: &str, dir_name: &str) -> Option<Located> {
    let dir = layout.forest_dir(forest).join(dir_name);
    dir.is_dir().then(|| Located::Workforest {
        forest: forest.to_string(),
        dir,
    })
}
