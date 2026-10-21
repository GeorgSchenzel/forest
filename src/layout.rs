//! The fixed on-disk layout: root, repos, forests, names and cwd context.
//!
//! A repo is `<root>/repos/<name>/`, a normal clone whose own checkout is its home.
//! A forest is any dir `<root>/forests/<name>/`; its worktrees are at
//! `<root>/forests/<name>/<repo>/`, so nothing else is stored.

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, fail};

pub const REPOS: &str = "repos";
pub const FORESTS: &str = "forests";

/// Where cwd is.
#[derive(Debug, PartialEq)]
pub enum Context {
    /// In a repo's home checkout.
    Repo(String),
    /// In a forest dir, or in one of its worktrees.
    Forest(String),
}

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    /// Root from `--root`, else `FOREST_ROOT`, else `~/projects`.
    pub fn new(flag: Option<PathBuf>) -> Result<Self> {
        let root = match flag {
            Some(p) => p,
            None => match env::var_os("FOREST_ROOT") {
                Some(v) if !v.is_empty() => PathBuf::from(v),
                _ => {
                    let home = env::var_os("HOME").ok_or_else(|| fail!("HOME is not set"))?;
                    PathBuf::from(home).join("projects")
                }
            },
        };
        let root = if root.is_absolute() {
            root
        } else {
            env::current_dir()?.join(root)
        };
        let root = canonical(&root);
        Ok(Layout { root })
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.root.join(REPOS)
    }

    pub fn forests_dir(&self) -> PathBuf {
        self.root.join(FORESTS)
    }

    pub fn repo_dir(&self, project: &str) -> PathBuf {
        self.repos_dir().join(project)
    }

    pub fn forest_dir(&self, forest: &str) -> PathBuf {
        self.forests_dir().join(forest)
    }

    pub fn is_project(&self, name: &str) -> bool {
        valid_name(name).is_ok() && self.repo_dir(name).join(".git").is_dir()
    }

    pub fn is_forest(&self, name: &str) -> bool {
        valid_name(name).is_ok() && self.forest_dir(name).is_dir()
    }

    pub fn projects(&self) -> Result<Vec<String>> {
        names_in(&self.repos_dir(), |n| self.is_project(n))
    }

    pub fn forests(&self) -> Result<Vec<String>> {
        names_in(&self.forests_dir(), |n| self.is_forest(n))
    }

    /// The forest a worktree at `path` belongs to: `<root>/forests/<forest>/<repo>`.
    pub fn forest_of(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.forests_dir()).ok()?;
        let parts: Vec<_> = rel.iter().collect();
        (parts.len() == 2).then(|| parts[0].to_string_lossy().into_owned())
    }

    /// The repo or forest cwd is in, if any.
    pub fn context(&self) -> Option<Context> {
        let cwd = env::current_dir().ok()?.canonicalize().ok()?;
        let rel = cwd.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => s.to_str().map(String::from),
                _ => None,
            })
            .collect();
        let name = parts.get(1)?;
        match parts.first()?.as_str() {
            REPOS => self.is_project(name).then(|| Context::Repo(name.clone())),
            FORESTS => self.is_forest(name).then(|| Context::Forest(name.clone())),
            _ => None,
        }
    }
}

fn names_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !dir.is_dir() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir)? {
        if let Some(name) = entry?.file_name().to_str()
            && keep(name)
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// `p` with symlinks resolved, or as given when it doesn't exist (paths compare equal
/// only when built the same way).
pub fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Flat dir name for a branch: `/` and anything outside `[A-Za-z0-9._-]` become `-`,
/// and so does a leading `.`.
pub fn slug(branch: &str) -> String {
    let mut s: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.starts_with('.') {
        s.replace_range(0..1, "-");
    }
    s
}

/// A repo or forest name: one plain path component.
pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) {
        return Err(fail!(
            "invalid name '{name}': must not be empty, start with '.', or contain '/' or ':'"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("feat/x"), "feat-x");
        assert_eq!(slug("main"), "main");
        assert_eq!(slug(".hidden/a b"), "-hidden-a-b");
        assert_eq!(slug("v1.2_rc-1"), "v1.2_rc-1");
        assert_eq!(slug("ü/x"), "--x");
        assert_eq!(slug(&slug("a/.b")), slug("a/.b"));
    }

    #[test]
    fn names() {
        assert!(valid_name("backend").is_ok());
        assert!(valid_name("_x").is_ok());
        assert!(valid_name(".x").is_err());
        assert!(valid_name("a:b").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("").is_err());
    }
}
