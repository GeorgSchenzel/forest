//! `ls`: projects and forests with their worktrees and state.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::Output;
use crate::error::Result;
use crate::git::git;
use crate::layout::{FORESTS, Layout, Name};
use crate::project::{default_branch, origin_url, worktrees};

#[derive(Serialize)]
struct Row {
    project: String,
    branch: Option<String>,
    dir: String,
    forest: Option<String>,
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
}

pub fn ls(layout: &Layout, name: Option<&str>) -> Result<Output> {
    let (projects, forests) = match name.map(|n| layout.resolve_name(n)).transpose()? {
        None => (layout.projects()?, layout.forests()?),
        Some(Name::Project(p)) => (vec![p], vec![]),
        Some(Name::Forest(f)) => (vec![], vec![f]),
    };
    // Rows come from the projects; a forest-only listing still needs every project's.
    let sources = if forests.is_empty() {
        projects.clone()
    } else {
        layout.projects()?
    };
    let mut rows = Vec::new();
    for p in &sources {
        rows.extend(project_rows(layout, p)?);
    }
    // Keep only what is listed before running `git status` in each worktree.
    rows.retain(|r| {
        projects.contains(&r.project) || r.forest.as_ref().is_some_and(|f| forests.contains(f))
    });
    for row in rows.iter_mut().filter(|r| !r.missing) {
        fill_status(row);
    }

    let mut text = String::new();
    let mut projects_json = Vec::new();
    for p in &projects {
        let bare = layout.bare(p);
        let default = default_branch(&bare).ok();
        let origin = origin_url(&bare);
        text.push_str(&format!(
            "{p}  (default: {}, {})\n",
            default.as_deref().unwrap_or("?"),
            origin
                .as_deref()
                .map_or("no origin".to_string(), |o| format!("origin {o}")),
        ));
        let mine: Vec<&Row> = rows.iter().filter(|r| &r.project == p).collect();
        if mine.is_empty() {
            text.push_str("  (no worktrees)\n");
        }
        let table = mine
            .iter()
            .map(|r| {
                let place = match &r.forest {
                    Some(f) => format!("forest {f}"),
                    None => r.dir.clone(),
                };
                vec![place, r.branch_str().into(), r.head_str().into(), r.state()]
            })
            .collect();
        text.push_str(&align(table, 2));
        projects_json.push(json!({
            "name": p,
            "path": layout.project_dir(p),
            "default_branch": default,
            "origin": origin,
        }));
    }

    let mut forests_json = Vec::new();
    for f in &forests {
        let path = layout.forest_dir(f);
        let mut members: Vec<&Row> = rows
            .iter()
            .filter(|r| r.forest.as_ref() == Some(f))
            .collect();
        members.sort_by(|a, b| a.project.cmp(&b.project));
        let branch = members.iter().find_map(|r| r.branch.clone());
        text.push_str(&format!("{f}  {}\n", branch.as_deref().unwrap_or("?")));
        if members.is_empty() {
            text.push_str("  (no worktrees)\n");
        }
        let table = members
            .iter()
            .map(|r| {
                let mut state = r.state();
                // Members normally share the forest's branch; say so when one doesn't.
                if r.branch != branch {
                    state = format!("{state}, on {}", r.branch_str());
                }
                vec![r.project.clone(), r.head_str().into(), state]
            })
            .collect();
        text.push_str(&align(table, 2));
        let names: Vec<&str> = members.iter().map(|r| r.project.as_str()).collect();
        forests_json.push(json!({
            "name": f,
            "path": path,
            "branch": branch,
            "projects": names,
        }));
    }
    if projects.is_empty() && forests.is_empty() {
        text.push_str(&format!("no projects in {}\n", layout.root.display()));
    }

    Ok(Output::new(
        text,
        json!({
            "projects": projects_json,
            "forests": forests_json,
            "worktrees": serde_json::to_value(&rows).unwrap_or(Value::Null),
        }),
    ))
}

fn project_rows(layout: &Layout, project: &str) -> Result<Vec<Row>> {
    let forests_dir = layout.root.join(FORESTS);
    let mut rows = Vec::new();
    for w in worktrees(&layout.bare(project))? {
        // <root>/_forests/<forest>/<project>
        let in_forest = w
            .path
            .strip_prefix(&forests_dir)
            .ok()
            .map(|rel| {
                rel.iter()
                    .map(|c| c.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .filter(|parts| parts.len() == 2);
        let forest = in_forest.map(|parts| parts[0].clone());
        let dir = w.dir_name();
        let row = Row {
            project: project.to_string(),
            branch: w.branch,
            dir,
            forest,
            head: None,
            dirty: None,
            ahead: None,
            behind: None,
            upstream: None,
            upstream_gone: false,
            missing: !w.path.is_dir(),
            path: w.path,
            error: None,
        };
        rows.push(row);
    }
    rows.sort_by(|a, b| (&a.forest, &a.dir).cmp(&(&b.forest, &b.dir)));
    Ok(rows)
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
fn align(rows: Vec<Vec<String>>, indent: usize) -> String {
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
