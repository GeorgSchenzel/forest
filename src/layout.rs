//! The fixed on-disk layout: root, projects, forests, names, targets and cwd context.
//!
//! A project is `<root>/<name>/` with a bare repo in `.bare`. A forest is any dir
//! `<root>/_forests/<name>/`; its members are the worktrees in it, so nothing else is
//! stored.

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, fail};

pub const FORESTS: &str = "_forests";
pub const BARE: &str = ".bare";

#[derive(Debug, Clone, PartialEq)]
pub enum Name {
    Project(String),
    Forest(String),
}

/// Where cwd is: a project or forest, plus the first path component below it
/// (a worktree dir, or a forest's member), if any.
pub struct Context {
    pub name: Name,
    pub sub: Option<String>,
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

    pub fn project_dir(&self, project: &str) -> PathBuf {
        self.root.join(project)
    }

    pub fn bare(&self, project: &str) -> PathBuf {
        self.root.join(project).join(BARE)
    }

    pub fn forest_dir(&self, forest: &str) -> PathBuf {
        self.root.join(FORESTS).join(forest)
    }

    pub fn is_project(&self, name: &str) -> bool {
        valid_project_name(name).is_ok() && self.bare(name).is_dir()
    }

    pub fn is_forest(&self, name: &str) -> bool {
        valid_forest_name(name).is_ok() && self.forest_dir(name).is_dir()
    }

    pub fn projects(&self) -> Result<Vec<String>> {
        self.names_in(&self.root, |n| self.is_project(n))
    }

    pub fn forests(&self) -> Result<Vec<String>> {
        self.names_in(&self.root.join(FORESTS), |n| self.is_forest(n))
    }

    fn names_in(&self, dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<String>> {
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

    /// Resolves a project or forest name; a name that is both is an error.
    pub fn resolve_name(&self, name: &str) -> Result<Name> {
        match (self.is_project(name), self.is_forest(name)) {
            (true, true) => Err(fail!(
                "'{name}' is both a project and a forest; rename the forest"
            )),
            (true, false) => Ok(Name::Project(name.to_string())),
            (false, true) => Ok(Name::Forest(name.to_string())),
            (false, false) => Err(fail!(
                "no project or forest named '{name}' in {}",
                self.root.display()
            )),
        }
    }

    /// The project or forest cwd is in, if any.
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
        let first = parts.first()?;
        if first == FORESTS {
            let forest = parts.get(1)?;
            return self.is_forest(forest).then(|| Context {
                name: Name::Forest(forest.clone()),
                sub: parts.get(2).cloned(),
            });
        }
        self.is_project(first).then(|| Context {
            name: Name::Project(first.clone()),
            sub: parts.get(1).cloned(),
        })
    }

    /// Splits `<project>:<branch>`, or `:<branch>` in the project cwd is in; `None`
    /// for a word without `:`, which names a forest.
    pub fn split_target(&self, s: &str) -> Result<Option<(String, String)>> {
        let Some((project, branch)) = s.split_once(':') else {
            return Ok(None);
        };
        if branch.is_empty() {
            return Err(fail!("invalid target '{s}': expected <project>:<branch>"));
        }
        if project.is_empty() {
            return match self.context_project() {
                Some(p) => Ok(Some((p, branch.to_string()))),
                None => Err(fail!(
                    "'{s}' needs a project: run it inside one, or write <project>{s}"
                )),
            };
        }
        if !self.is_project(project) {
            return Err(fail!(
                "no project named '{project}' in {}",
                self.root.display()
            ));
        }
        Ok(Some((project.to_string(), branch.to_string())))
    }

    /// The forest a worktree at `path` belongs to: `<root>/_forests/<forest>/<project>`.
    pub fn forest_of(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.root.join(FORESTS)).ok()?;
        let parts: Vec<_> = rel.iter().collect();
        (parts.len() == 2).then(|| parts[0].to_string_lossy().into_owned())
    }

    /// The project cwd is in, if any.
    pub fn context_project(&self) -> Option<String> {
        match self.context()?.name {
            Name::Project(p) => Some(p),
            Name::Forest(_) => None,
        }
    }
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

pub fn valid_project_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('_')
        || name.starts_with('.')
        || name.contains(['/', '\\', ':'])
    {
        return Err(fail!(
            "invalid project name '{name}': must not be empty, start with '_' or '.', or contain '/' or ':'"
        ));
    }
    Ok(())
}

pub fn valid_forest_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) {
        return Err(fail!(
            "invalid forest name '{name}': must not be empty, start with '.', or contain '/' or ':'"
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
    fn project_names() {
        assert!(valid_project_name("backend").is_ok());
        assert!(valid_project_name("_forests").is_err());
        assert!(valid_project_name(".x").is_err());
        assert!(valid_project_name("a:b").is_err());
        assert!(valid_project_name("").is_err());
    }
}
