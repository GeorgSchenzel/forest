//! `push`: push the branches of a forest's worktrees to origin, tracking them.

use serde_json::json;

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::error::{Result, fail};
use crate::git::git_loud;
use crate::layout::Layout;
use crate::project::members;

pub fn push(layout: &Layout, target: Option<&str>, project: Option<&str>) -> Result<Output> {
    let located = locate(layout, target, project)?;
    let identity = located.identity(layout)?;
    let items = match located {
        Located::Forest { name, .. } => members(layout, &name)?,
        Located::Worktree {
            project, worktree, ..
        } => vec![(project, worktree)],
    };
    if items.is_empty() {
        return Err(fail!("{} has no worktrees", identity.path.display()));
    }
    let mut text = String::new();
    let mut pushes = Vec::new();
    let mut errors = Vec::new();
    for (project, worktree) in items {
        let Some(branch) = worktree.branch.as_deref() else {
            errors.push(format!("{project} is detached"));
            pushes.push(json!({"project": project, "branch": null, "error": "detached"}));
            continue;
        };
        // Run in the repo: a branch is shared by all its worktrees.
        let repo = layout.repo_dir(&project);
        match git_loud(&repo, ["push", "-u", "origin", branch]) {
            Ok(()) => {
                text.push_str(&format!("pushed {branch} of {project} to origin\n"));
                pushes.push(json!({"project": project, "branch": branch}));
            }
            Err(e) => {
                errors.push(format!("{project}: {e}"));
                pushes.push(json!({
                    "project": project,
                    "branch": branch,
                    "error": e.to_string(),
                }));
            }
        }
    }
    // One repo failing doesn't stop the others; they were pushed already.
    for e in &errors {
        eprintln!("forest: {e}");
    }
    let mut out = Output::new(
        text,
        identity.with(json!({"pushes": pushes, "errors": errors})),
    );
    if !errors.is_empty() {
        out.code = 1;
    }
    Ok(out)
}
