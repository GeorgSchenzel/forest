//! `init` and `clone`: create a repo at `<root>/repos/<name>/`.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::Output;
use crate::error::{Result, fail};
use crate::git::{git, git_loud};
use crate::layout::{Layout, valid_name};
use crate::project::{check_branch_name, configure, default_branch};

pub fn init(layout: &Layout, name: &str, branch: Option<&str>) -> Result<Output> {
    if let Some(b) = branch {
        check_branch_name(b)?;
    }
    create_repo(layout, name, |parent| {
        let mut args = vec!["init", "--quiet"];
        let initial;
        if let Some(b) = branch {
            initial = format!("--initial-branch={b}");
            args.push(&initial);
        }
        args.push(name);
        git(parent, args).map(|_| ())
    })
}

pub fn clone(layout: &Layout, url: &str, name: Option<&str>) -> Result<Output> {
    let name = match name {
        Some(n) => n.to_string(),
        None => name_from_url(url)
            .ok_or_else(|| fail!("cannot derive a repo name from '{url}'; pass one"))?,
    };
    // A local path must stay valid when git runs in the repos dir.
    let local = Path::new(url);
    let url = if local.exists() {
        local.canonicalize()?.to_string_lossy().into_owned()
    } else {
        url.to_string()
    };
    create_repo(layout, &name, |parent| {
        git_loud(parent, ["clone", &url, &name])
    })
}

/// Runs `setup` in the repos dir to make `<name>`, and removes it again on failure.
fn create_repo(
    layout: &Layout,
    name: &str,
    setup: impl FnOnce(&Path) -> Result<()>,
) -> Result<Output> {
    valid_name(name)?;
    let dir = layout.repo_dir(name);
    if dir.exists() {
        return Err(fail!("{} already exists", dir.display()));
    }
    let parent = layout.repos_dir();
    fs::create_dir_all(&parent)?;
    let result = setup(&parent).and_then(|()| configure(&dir));
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    let dir = dir.canonicalize()?;
    let branch = default_branch(&dir).ok();
    Ok(Output::new(
        format!("{}\n", dir.display()),
        json!({"project": name, "branch": branch, "path": dir}),
    ))
}

/// `git@host:org/shop.git` → `shop`, `/src/shop/.git` → `shop`.
fn name_from_url(url: &str) -> Option<String> {
    let t = url.trim_end_matches('/');
    let t = t.strip_suffix(".git").unwrap_or(t).trim_end_matches('/');
    let name = t.rsplit(['/', ':', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::name_from_url;

    #[test]
    fn names() {
        assert_eq!(
            name_from_url("git@github.com:org/shop.git").unwrap(),
            "shop"
        );
        assert_eq!(name_from_url("https://x.org/a/b/").unwrap(), "b");
        assert_eq!(name_from_url("/src/shop/.git").unwrap(), "shop");
        assert_eq!(name_from_url("../shop").unwrap(), "shop");
        assert!(name_from_url("/").is_none());
    }
}
