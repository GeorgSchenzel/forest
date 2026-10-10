//! `define`: create a forest, or change its project list.

use serde_json::json;

use crate::Output;
use crate::error::{Result, fail};
use crate::layout::{Layout, valid_forest_name};

pub fn define(
    layout: &Layout,
    name: &str,
    projects: &[String],
    add: &[String],
    remove: &[String],
) -> Result<Output> {
    valid_forest_name(name)?;
    let exists = layout.is_forest(name);
    let list = if exists {
        if !projects.is_empty() {
            return Err(fail!(
                "forest '{name}' already exists; change it with --add/--remove"
            ));
        }
        if add.is_empty() && remove.is_empty() {
            return Err(fail!("nothing to change: give --add or --remove"));
        }
        let mut list = layout.forest_projects(name)?;
        for p in remove {
            if !list.contains(p) {
                return Err(fail!("'{p}' is not in forest '{name}'"));
            }
        }
        list.retain(|p| !remove.contains(p));
        check_projects(layout, add)?;
        push_unique(&mut list, add);
        list
    } else {
        if !add.is_empty() || !remove.is_empty() {
            return Err(fail!(
                "no forest named '{name}'; create it with: forest define {name} <project>..."
            ));
        }
        if projects.is_empty() {
            return Err(fail!("a forest needs at least one project"));
        }
        if layout.is_project(name) {
            return Err(fail!(
                "a project named '{name}' exists; a forest can't share its name"
            ));
        }
        check_projects(layout, projects)?;
        let mut list = Vec::new();
        push_unique(&mut list, projects);
        list
    };
    if list.is_empty() {
        return Err(fail!("a forest needs at least one project"));
    }

    let dir = layout.write_forest_projects(name, &list)?;
    if !remove.is_empty() {
        eprintln!(
            "worktrees of {} in existing workforests are kept; `forest ls <project>` shows them",
            remove.join(", ")
        );
    }
    Ok(Output::new(
        format!("{name}  [{}]\n", list.join(", ")),
        json!({"name": name, "path": dir, "projects": list, "created": !exists}),
    ))
}

fn check_projects(layout: &Layout, projects: &[String]) -> Result<()> {
    let missing: Vec<&str> = projects
        .iter()
        .filter(|p| !layout.is_project(p))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(fail!("no such project: {}", missing.join(", ")))
    }
}

fn push_unique(list: &mut Vec<String>, items: &[String]) {
    for p in items {
        if !list.contains(p) {
            list.push(p.clone());
        }
    }
}
