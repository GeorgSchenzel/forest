//! `path`, and resolving a target (or cwd) to a worktree or workforest.

use std::path::PathBuf;

use serde_json::json;

use crate::Output;
use crate::error::{Result, fail};
use crate::layout::{Layout, Name, slug};
use crate::project::{Worktree, worktrees};

pub enum Located {
    Worktree { project: String, worktree: Worktree },
    Workforest { dir: PathBuf },
}

impl Located {
    pub fn path(&self) -> &PathBuf {
        match self {
            Located::Worktree { worktree, .. } => &worktree.path,
            Located::Workforest { dir, .. } => dir,
        }
    }
}

pub fn path(layout: &Layout, target: Option<&str>) -> Result<Output> {
    let located = locate(layout, target)?;
    let path = located.path();
    Ok(Output::new(
        format!("{}\n", path.display()),
        json!({"path": path}),
    ))
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
    dir.is_dir().then_some(Located::Workforest { dir })
}
