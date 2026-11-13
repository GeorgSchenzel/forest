//! `add`: put more repos into the forest you are in.

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::new::new;
use crate::cmd::path::Identity;
use crate::error::{Result, fail, refused};
use crate::layout::{Context, Layout};
use crate::project::{members, one_branch};

/// Adds a worktree of each repo to the forest of the cwd, on the branch its
/// worktrees share. The same path as `forest new <that branch> <repo>…`: a repo
/// of the forest already is reported, never an error.
pub fn add(layout: &Layout, repos: &[String], no_fetch: bool) -> Result<Output> {
    let Some(Context::Forest(forest)) = layout.context() else {
        return Err(refused!(
            "not inside a forest; `forest add` adds repos to the forest you are in"
        ));
    };
    let members = members(layout, &forest)?;
    let branch = one_branch(&forest, &members)?;

    let mut unique: Vec<&String> = Vec::new();
    for r in repos {
        if !layout.is_project(r) {
            return Err(fail!("no repo named '{r}'"));
        }
        if !unique.contains(&r) {
            unique.push(r);
        }
    }
    let (present, todo): (Vec<&String>, Vec<&String>) = unique
        .into_iter()
        .partition(|r| members.iter().any(|(p, _)| p.as_str() == r.as_str()));

    let dir = layout.forest_dir(&forest);
    if todo.is_empty() {
        for p in &present {
            eprintln!("{p}: already in forest '{forest}'");
        }
        return Ok(Output::new(
            format!("{}\n", dir.display()),
            Identity::forest(&forest, &dir, Some(branch))
                .with(json!({"created": false, "worktrees": [], "present": present})),
        ));
    }
    let mut out = new(
        layout,
        Some(&branch),
        &todo.into_iter().cloned().collect::<Vec<_>>(),
        None,
        None,
        no_fetch,
    )?;
    for p in &present {
        eprintln!("{p}: already in forest '{forest}'");
    }
    if let Value::Object(fields) = &mut out.json {
        fields.insert("present".into(), json!(present));
    }
    Ok(out)
}
