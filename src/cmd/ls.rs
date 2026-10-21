//! `ls`: repos and forests with their worktrees and state.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::error::Result;
use crate::git::git;
use crate::layout::{Layout, slug};
use crate::project::{all_worktrees, default_branch, origin_url};

#[derive(Serialize)]
struct Row {
    project: String,
    branch: Option<String>,
    /// The forest it is in; `None` for a repo's home and worktrees outside the layout.
    forest: Option<String>,
    /// The repo's own checkout.
    home: bool,
    path: PathBuf,
    head: Option<String>,
    dirty: Option<bool>,
    ahead: Option<u32>,
    behind: Option<u32>,
    upstream: Option<String>,
    upstream_gone: bool,
    missing: bool,
    /// Why `git status` failed in this worktree.
    error: Option<String>,
}

impl Row {
    fn state(&self) -> String {
        if self.missing {
            return "missing".into();
        }
        if let Some(e) = &self.error {
            return format!("error: {}", e.lines().next().unwrap_or_default());
        }
        let mut parts = Vec::new();
        if self.dirty == Some(true) {
            parts.push("dirty".to_string());
        }
        match (&self.upstream, self.upstream_gone) {
            (None, _) => parts.push("no-upstream".into()),
            (Some(_), true) => parts.push("upstream gone".into()),
            (Some(_), false) => {
                if let Some(n) = self.ahead.filter(|n| *n > 0) {
                    parts.push(format!("ahead {n}"));
                }
                if let Some(n) = self.behind.filter(|n| *n > 0) {
                    parts.push(format!("behind {n}"));
                }
            }
        }
        if parts.is_empty() {
            "clean".into()
        } else {
            parts.join(", ")
        }
    }

    fn branch_str(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }

    fn head_str(&self) -> &str {
        self.head.as_deref().unwrap_or("(unborn)")
    }

    fn elsewhere(&self) -> bool {
        !self.home && self.forest.is_none()
    }
}

/// Everything, or one repo (its home, the forests it is in and its other worktrees),
/// or one forest.
pub fn ls(layout: &Layout, name: Option<&str>) -> Result<Output> {
    let mut rows: Vec<Row> = all_worktrees(layout)?
        .into_iter()
        .map(|(project, w)| Row {
            project,
            branch: w.branch,
            forest: layout.forest_of(&w.path),
            home: w.home,
            head: None,
            dirty: None,
            ahead: None,
            behind: None,
            upstream: None,
            upstream_gone: false,
            missing: !w.path.is_dir(),
            path: w.path,
            error: None,
        })
        .collect();
    let (projects, forests) = match name {
        None => (layout.projects()?, layout.forests()?),
        Some(n) => match locate(layout, Some(n), None)? {
            Located::Forest { name, .. } => (vec![], vec![name]),
            Located::Worktree { project, .. } => {
                let mut forests: Vec<String> = rows
                    .iter()
                    .filter(|r| r.project == project)
                    .filter_map(|r| r.forest.clone())
                    .collect();
                forests.sort();
                forests.dedup();
                (vec![project], forests)
            }
        },
    };
    // Keep only what is listed before running `git status` in each worktree.
    rows.retain(|r| match &r.forest {
        Some(f) => forests.contains(f),
        None => projects.contains(&r.project),
    });
    for row in rows.iter_mut().filter(|r| !r.missing) {
        fill_status(row);
    }
    rows.sort_by(|a, b| (&a.forest, &a.project, &a.path).cmp(&(&b.forest, &b.project, &b.path)));

    let mut text = String::new();
    let mut repos_json = Vec::new();
    let mut table = Vec::new();
    for p in &projects {
        let repo = layout.repo_dir(p);
        let default = default_branch(&repo).ok();
        let origin = origin_url(&repo);
        match rows.iter().find(|r| r.home && &r.project == p) {
            Some(r) => table.push(vec![
                p.clone(),
                r.branch_str().into(),
                r.head_str().into(),
                r.state(),
            ]),
            None => table.push(vec![p.clone(), "(no checkout)".into()]),
        }
        repos_json.push(json!({
            "name": p,
            "path": repo,
            "default_branch": default,
            "origin": origin,
        }));
    }
    if !table.is_empty() {
        text.push_str("repos\n");
        text.push_str(&align(table, 2));
    }

    let mut forests_json = Vec::new();
    let mut table = Vec::new();
    for f in &forests {
        let members: Vec<&Row> = rows
            .iter()
            .filter(|r| r.forest.as_ref() == Some(f))
            .collect();
        let branch = members.iter().find_map(|r| r.branch.clone());
        if members.is_empty() {
            table.push(vec![f.clone(), String::new(), "(no worktrees)".into()]);
        }
        for (i, r) in members.iter().enumerate() {
            // The branch only when the name doesn't already say it.
            let (name, shown) = if i == 0 {
                let shown = match &branch {
                    Some(b) if slug(b) == *f => String::new(),
                    Some(b) => b.clone(),
                    None => "(detached)".into(),
                };
                (f.clone(), shown)
            } else {
                (String::new(), String::new())
            };
            let mut state = r.state();
            // Worktrees normally share the forest's branch; say so when one doesn't.
            if r.branch != branch {
                state = format!("{state}, on {}", r.branch_str());
            }
            table.push(vec![
                name,
                shown,
                r.project.clone(),
                r.head_str().into(),
                state,
            ]);
        }
        let names: Vec<&str> = members.iter().map(|r| r.project.as_str()).collect();
        forests_json.push(json!({
            "name": f,
            "path": layout.forest_dir(f),
            "branch": branch,
            "projects": names,
        }));
    }
    if !table.is_empty() {
        text.push_str("forests\n");
        text.push_str(&align(table, 2));
    }

    let table: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| r.elsewhere())
        .map(|r| {
            vec![
                r.project.clone(),
                r.branch_str().into(),
                r.path.display().to_string(),
                r.state(),
            ]
        })
        .collect();
    if !table.is_empty() {
        text.push_str("outside the layout\n");
        text.push_str(&align(table, 2));
    }
    if projects.is_empty() && forests.is_empty() {
        text.push_str(&format!("no repos in {}\n", layout.repos_dir().display()));
    }

    Ok(Output::new(
        text,
        json!({
            "repos": repos_json,
            "forests": forests_json,
            "worktrees": serde_json::to_value(&rows).unwrap_or(Value::Null),
        }),
    ))
}

/// One `git status` per worktree gives head, upstream, ahead/behind and dirtiness.
/// A failure is recorded on the row, so one broken worktree doesn't hide the rest.
fn fill_status(row: &mut Row) {
    let out = match git(&row.path, ["status", "--porcelain=v2", "--branch"]) {
        Ok(out) => out,
        Err(e) => {
            row.error = Some(e.to_string());
            return;
        }
    };
    let mut dirty = false;
    let mut has_ab = false;
    for line in out.lines() {
        if let Some(oid) = line.strip_prefix("# branch.oid ") {
            if oid != "(initial)" {
                row.head = Some(oid.chars().take(7).collect());
            }
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            row.upstream = Some(up.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            has_ab = true;
            let mut it = ab.split_whitespace();
            row.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok());
            row.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if !line.starts_with('#') {
            dirty = true;
        }
    }
    row.dirty = Some(dirty);
    row.upstream_gone = row.upstream.is_some() && !has_ab;
}

/// Left-aligned columns, two spaces apart, trailing blanks trimmed.
pub fn align(rows: Vec<Vec<String>>, indent: usize) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = " ".repeat(indent);
        for (c, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if c + 1 < row.len() {
                line.push_str(&" ".repeat(widths[c] - cell.chars().count() + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}
