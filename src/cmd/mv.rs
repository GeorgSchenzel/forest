//! `mv`: rename a forest. Only its dir changes; branches stay as they are.

use std::env;
use std::fs;

use serde_json::json;

use crate::Output;
use crate::cmd::new::check_new_name;
use crate::cmd::path::{Identity, Located, forest_branch, locate};
use crate::error::{Result, fail};
use crate::layout::{Layout, canonical};
use crate::project::{all_worktrees, worktree_repair};

pub fn mv(layout: &Layout, target: &str, new_name: &str) -> Result<Output> {
    let Located::Forest { name, dir } = locate(layout, Some(target), None)? else {
        return Err(fail!("'{target}' is a repo; mv renames forests"));
    };
    if name == new_name {
        return Err(fail!("forest '{name}' already has that name"));
    }
    let all = all_worktrees(layout)?;
    check_new_name(layout, &all, new_name, Some(&name))?;
    let cwd_inside = env::current_dir().is_ok_and(|c| canonical(&c).starts_with(&dir));

    let new_dir = layout.forest_dir(new_name);
    fs::rename(&dir, &new_dir).map_err(|e| {
        fail!(
            "cannot move {} to {}: {e}",
            dir.display(),
            new_dir.display()
        )
    })?;
    // git still points at the old paths until each repo learns the new ones.
    let mut errors = Vec::new();
    for (project, w) in &all {
        if layout.forest_of(&w.path).as_deref() == Some(name.as_str()) {
            let moved = new_dir.join(project);
            if let Err(e) = worktree_repair(&layout.repo_dir(project), &[moved]) {
                errors.push(format!("{project}: {e}"));
            }
        }
    }
    if !errors.is_empty() {
        return Err(fail!(
            "moved {} to {}, but git couldn't follow:\n{}",
            dir.display(),
            new_dir.display(),
            errors.join("\n")
        ));
    }

    let mut out = Output::new(
        format!("{}\n", new_dir.display()),
        Identity::forest(new_name, &new_dir, forest_branch(layout, new_name)?)
            .with(json!({"renamed_from": name})),
    );
    if cwd_inside {
        out.warnings.push(format!(
            "the current directory was inside {}; cd to {}",
            dir.display(),
            new_dir.display()
        ));
    }
    Ok(out)
}
