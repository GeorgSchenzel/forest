//! `init` and `clone`: create a project.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::error::{Result, fail};
use crate::git::{git, git_loud, relative_paths};
use crate::layout::{BARE, Layout, slug, valid_project_name};
use crate::project::{check_branch_name, default_branch, worktree_add};

pub fn init(layout: &Layout, name: &str, branch: Option<&str>) -> Result<Output> {
    if let Some(b) = branch {
        check_branch_name(b)?;
    }
    create_project(layout, name, |dir, bare| {
        let mut args = vec!["init", "--bare", "--quiet"];
        let initial;
        if let Some(b) = branch {
            initial = format!("--initial-branch={b}");
            args.push(&initial);
        }
        args.push(BARE);
        git(dir, args)?;
        configure(bare)?;
        let default = default_branch(bare)?;
        let worktree = dir.join(slug(&default));
        worktree_add(bare, &["--orphan", "-b", &default], &worktree, &[])?;
        Ok((default, worktree))
    })
}

pub fn clone(layout: &Layout, url: &str, name: Option<&str>) -> Result<Output> {
    let name = match name {
        Some(n) => n.to_string(),
        None => name_from_url(url)
            .ok_or_else(|| fail!("cannot derive a project name from '{url}'; pass one"))?,
    };
    // A local path must stay valid when git runs inside the bare repo.
    let local = Path::new(url);
    let url = if local.exists() {
        local.canonicalize()?.to_string_lossy().into_owned()
    } else {
        url.to_string()
    };
    create_project(layout, &name, |dir, bare| {
        git(dir, ["init", "--bare", "--quiet", BARE])?;
        configure(bare)?;
        git(bare, ["remote", "add", "origin", &url])?;
        git_loud(bare, ["fetch", "origin"])?;
        git(bare, ["remote", "set-head", "origin", "--auto"]).map_err(|e| {
            fail!("cannot determine the remote's default branch (is it empty?): {e}")
        })?;
        let default = default_branch(bare)?;
        git(
            bare,
            ["symbolic-ref", "HEAD", &format!("refs/heads/{default}")],
        )?;
        let worktree = dir.join(slug(&default));
        let upstream = format!("origin/{default}");
        worktree_add(bare, &["--track", "-b", &default], &worktree, &[&upstream])?;
        Ok((default, worktree))
    })
}

/// Creates `<root>/<name>/`, runs `setup` in it, and removes it again if setup fails.
fn create_project(
    layout: &Layout,
    name: &str,
    setup: impl FnOnce(&Path, &Path) -> Result<(String, PathBuf)>,
) -> Result<Output> {
    valid_project_name(name)?;
    if layout.is_forest(name) {
        return Err(fail!(
            "a forest named '{name}' exists; a project can't share its name"
        ));
    }
    let dir = layout.project_dir(name);
    if dir.exists() {
        return Err(fail!("{} already exists", dir.display()));
    }
    fs::create_dir_all(&dir)?;
    let dir = dir.canonicalize()?;
    match setup(&dir, &dir.join(BARE)) {
        Ok((branch, worktree)) => Ok(Output::new(
            format!("{}\n", worktree.display()),
            json!({"project": name, "branch": branch, "path": worktree}),
        )),
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

fn configure(bare: &Path) -> Result<()> {
    if relative_paths() {
        git(bare, ["config", "worktree.useRelativePaths", "true"])?;
    }
    Ok(())
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
