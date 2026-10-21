//! `path`, and resolving a target (or cwd) to a forest or a worktree.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::Output;
use crate::error::{Result, fail};
use crate::layout::{Context, Layout};
use crate::project::{Worktree, all_worktrees, forests_on, members};

pub enum Located {
    Forest {
        name: String,
        dir: PathBuf,
    },
    /// A repo's home, a forest's worktree, or one outside the layout.
    Worktree {
        project: String,
        worktree: Worktree,
        forest: Option<String>,
    },
}

/// What a forest or worktree is, the same however it was addressed. In JSON
/// output, next to the command's own fields.
#[derive(Serialize)]
pub struct Identity {
    /// `"forest"` or `"worktree"`.
    pub kind: &'static str,
    /// The forest, or the forest a worktree is in; `None` for a repo's home.
    pub forest: Option<String>,
    /// The worktree's repo; `None` for a forest.
    pub project: Option<String>,
    /// `None` when detached, or for a forest without a worktree on a branch.
    pub branch: Option<String>,
    pub path: PathBuf,
}

impl Identity {
    pub fn forest(name: &str, dir: &Path, branch: Option<String>) -> Self {
        Identity {
            kind: "forest",
            forest: Some(name.to_string()),
            project: None,
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
    pub fn path(&self) -> &Path {
        match self {
            Located::Forest { dir, .. } => dir,
            Located::Worktree { worktree, .. } => &worktree.path,
        }
    }

    pub fn identity(&self, layout: &Layout) -> Result<Identity> {
        Ok(match self {
            Located::Forest { name, dir } => {
                Identity::forest(name, dir, forest_branch(layout, name)?)
            }
            Located::Worktree {
                project,
                worktree,
                forest,
            } => Identity {
                kind: "worktree",
                forest: forest.clone(),
                project: Some(project.clone()),
                branch: worktree.branch.clone(),
                path: worktree.path.clone(),
            },
        })
    }
}

/// A forest with only one worktree means that worktree, so `cd` lands in the repo.
pub fn path(layout: &Layout, target: Option<&str>, project: Option<&str>) -> Result<Output> {
    let located = step_in(layout, locate(layout, target, project)?)?;
    Ok(Output::new(
        format!("{}\n", located.path().display()),
        located.identity(layout)?.with(json!({})),
    ))
}

/// The forest's worktree when it has exactly one, else the forest itself.
fn step_in(layout: &Layout, located: Located) -> Result<Located> {
    let Located::Forest { name, .. } = &located else {
        return Ok(located);
    };
    let mut members = members(layout, name)?;
    if members.len() != 1 {
        return Ok(located);
    }
    let (project, worktree) = members.remove(0);
    Ok(Located::Worktree {
        project,
        worktree,
        forest: Some(name.clone()),
    })
}

/// The branch of a forest's first worktree on one, in repo order.
pub fn forest_branch(layout: &Layout, forest: &str) -> Result<Option<String>> {
    Ok(members(layout, forest)?
        .into_iter()
        .find_map(|(_, w)| w.branch))
}

/// What a word can name.
#[derive(PartialEq)]
enum Match {
    /// A forest, by its name or by the branch of its worktrees.
    Forest(String),
    /// A repo's home.
    Repo(String),
}

/// A target is a forest (by name or branch) or a repo; with `project`, that repo's
/// worktree in the forest, or its worktree on the branch. Without a target, it is
/// the forest or repo cwd is in. A word that names two things is an error.
pub fn locate(layout: &Layout, target: Option<&str>, project: Option<&str>) -> Result<Located> {
    let Some(t) = target else {
        return match layout.context() {
            Some(Context::Forest(f)) => Ok(forest(layout, &f)),
            Some(Context::Repo(p)) => home(layout, &p),
            None => Err(fail!("not inside a repo or forest; give a target")),
        };
    };
    let all = all_worktrees(layout)?;
    let mut matches = Vec::new();
    if layout.is_forest(t) {
        matches.push(Match::Forest(t.to_string()));
    }
    for f in forests_on(&all, layout, t) {
        if !matches.contains(&Match::Forest(f.clone())) {
            matches.push(Match::Forest(f));
        }
    }
    if project.is_none() && layout.is_project(t) {
        matches.push(Match::Repo(t.to_string()));
    }
    if matches.len() > 1 {
        let shown: Vec<String> = matches
            .iter()
            .map(|m| match m {
                Match::Forest(f) => format!("forests/{f}"),
                Match::Repo(p) => format!("repos/{p}"),
            })
            .collect();
        return Err(fail!("'{t}' is ambiguous: {}", shown.join(", ")));
    }

    let Some(p) = project else {
        return match matches.pop() {
            Some(Match::Forest(f)) => Ok(forest(layout, &f)),
            Some(Match::Repo(p)) => home(layout, &p),
            None => Err(fail!("no forest, branch or repo named '{t}'")),
        };
    };
    if !layout.is_project(p) {
        return Err(fail!("no repo named '{p}'"));
    }
    // In the forest, else the repo's worktree on that branch (its home, say).
    let in_forest = match matches.pop() {
        Some(Match::Forest(f)) => Some(f),
        _ => None,
    };
    let dir = in_forest.as_ref().map(|f| layout.forest_dir(f).join(p));
    all.into_iter()
        .find(|(q, w)| {
            q == p
                && match &dir {
                    Some(d) => &w.path == d,
                    None => w.branch.as_deref() == Some(t),
                }
        })
        .map(|(project, worktree)| Located::Worktree {
            forest: layout.forest_of(&worktree.path),
            project,
            worktree,
        })
        .ok_or_else(|| match in_forest {
            Some(f) => fail!("forest '{f}' has no worktree of {p}"),
            None => fail!("{p} has no worktree on '{t}'"),
        })
}

fn forest(layout: &Layout, name: &str) -> Located {
    Located::Forest {
        name: name.to_string(),
        dir: layout.forest_dir(name),
    }
}

fn home(layout: &Layout, project: &str) -> Result<Located> {
    let repo = layout.repo_dir(project);
    let worktree = crate::project::worktrees(&repo)?
        .into_iter()
        .find(|w| w.home)
        .ok_or_else(|| fail!("{} has no checkout", repo.display()))?;
    Ok(Located::Worktree {
        project: project.to_string(),
        worktree,
        forest: None,
    })
}
