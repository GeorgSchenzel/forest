//! `migrate`: bring an existing repo into the layout.
//!
//! Accepts a normal clone (`.git` dir + working tree), a bare repo, or a project
//! that is nearly in the layout. The repo ends up at `<root>/<name>/.bare`, with
//! every worktree outside `_forests` at `<root>/<name>/<branch-dir>`. Working
//! files are moved, never checked out again, so uncommitted, untracked and
//! ignored files survive.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::error::{Error, Result, fail};
use crate::git::{git, git_out, relative_paths};
use crate::layout::{BARE, FORESTS, Layout, canonical, slug, valid_project_name};
use crate::project::{
    default_branch, live_worktrees, origin_url, ref_exists, worktree_add, worktrees,
};

/// Where the working files wait while the main worktree's dir is set up.
const STAGING: &str = ".forest-migrate";

/// Per-worktree state of an operation in progress, kept in the git dir of a
/// main worktree; it would be lost in the move.
const IN_PROGRESS: &[&str] = &[
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "rebase-merge",
    "rebase-apply",
];

enum Kind {
    /// `<dir>/.bare` already exists.
    Layout,
    /// `<dir>/.git` is a git dir; `main` is the branch of `<dir>` as a worktree.
    DotGit { main: Option<String> },
    /// `<dir>` itself is a bare repo.
    Bare,
}

pub fn migrate(layout: &Layout, path: &Path, name: Option<&str>) -> Result<Output> {
    let src = path
        .canonicalize()
        .map_err(|e| fail!("{}: {e}", path.display()))?;
    let name = match name {
        Some(n) => n.to_string(),
        None => name_from_dir(&src).ok_or_else(|| {
            fail!(
                "cannot derive a project name from {}; pass one",
                src.display()
            )
        })?,
    };
    valid_project_name(&name)?;
    if layout.is_forest(&name) {
        return Err(fail!(
            "a forest named '{name}' exists; a project can't share its name"
        ));
    }
    if layout.root.starts_with(&src) || src.starts_with(layout.root.join(FORESTS)) {
        return Err(fail!("{} can't be migrated", src.display()));
    }
    let dest = layout.project_dir(&name);
    if dest != src && dest.exists() {
        return Err(fail!("{} already exists", dest.display()));
    }

    let (kind, gitdir) = inspect(&src)?;
    let mut list = live_worktrees(&gitdir)?;
    let kind = match kind {
        Kind::DotGit { .. } => {
            let bare =
                git_out(&gitdir, ["config", "--bool", "core.bare"]).as_deref() == Some("true");
            let main = list.iter().position(|w| w.path == src).filter(|_| !bare);
            let main = match main {
                Some(i) => Some(check_main(&gitdir, &src, &list.remove(i).branch)?),
                None => None,
            };
            Kind::DotGit { main }
        }
        k => k,
    };
    if let Kind::Layout = kind
        && dest == src
        && list
            .iter()
            .all(|w| placed(layout, &dest, &w.path, w.branch.as_deref()))
        && !list.is_empty()
        && dot_git_link(&dest).is_none()
    {
        return Ok(Output::new(
            format!("{name} is already in the layout\n"),
            json!({"project": name, "path": dest, "actions": [], "skipped": []}),
        ));
    }

    let mut actions = Vec::new();
    let mut linked: Vec<PathBuf> = list.into_iter().map(|w| w.path).collect();
    if dest != src {
        fs::rename(&src, &dest)
            .map_err(|e| fail!("cannot move {} to {}: {e}", src.display(), dest.display()))?;
        actions.push(format!("moved {} to {}", src.display(), dest.display()));
        for p in &mut linked {
            *p = rebase(p, &src, &dest);
        }
    }
    let bare = dest.join(BARE);

    match kind {
        Kind::Layout => {}
        Kind::DotGit { main } => {
            fs::rename(dest.join(".git"), &bare)?;
            git(&bare, ["config", "core.bare", "true"])?;
            let _ = git_out(&bare, ["config", "--unset", "core.worktree"]);
            actions.push(format!("moved .git to {}", bare.display()));
            if let Some(branch) = main {
                let worktree = dest.join(slug(&branch));
                let staging = dest.join(STAGING);
                fs::create_dir(&staging)?;
                move_entries(&dest, &staging, |n| n != BARE && n != STAGING)?;
                worktree_add(&bare, &["--no-checkout"], &worktree, &[&branch])?;
                // The old index keeps what was staged and the stat cache.
                let admin = git(&worktree, ["rev-parse", "--absolute-git-dir"])?;
                let index = bare.join("index");
                if index.exists() {
                    fs::rename(&index, Path::new(&admin).join("index"))?;
                }
                move_entries(&staging, &worktree, |_| true)?;
                fs::remove_dir(&staging)?;
                for p in &mut linked {
                    *p = rebase(p, &dest, &worktree);
                }
                actions.push(format!("moved the working tree to {}", worktree.display()));
            }
        }
        Kind::Bare => {
            fs::create_dir(&bare)?;
            // Worktrees made inside the bare repo stay where they are.
            let keep: Vec<PathBuf> = linked.clone();
            move_entries(&dest, &bare, |n| {
                n != BARE && !keep.iter().any(|w| w.starts_with(dest.join(n)))
            })?;
            actions.push(format!("moved the bare repo into {}", bare.display()));
        }
    }

    if !linked.is_empty() {
        let mut args = vec!["worktree".to_string(), "repair".to_string()];
        if relative_paths() {
            args.push("--relative-paths".into());
        }
        args.extend(linked.iter().map(|p| p.to_string_lossy().into_owned()));
        git(&bare, args)?;
    }
    if relative_paths() {
        git(&bare, ["config", "worktree.useRelativePaths", "true"])?;
    }
    if dot_git_link(&dest).is_some() {
        fs::remove_file(dest.join(".git"))?;
        actions.push(format!("removed {}", dest.join(".git").display()));
    }
    if origin_url(&bare).is_some() && !ref_exists(&bare, "refs/remotes/origin/HEAD") {
        // Needs the network; without it the default falls back to our own HEAD.
        let _ = git_out(&bare, ["remote", "set-head", "origin", "--auto"]);
    }

    let skipped = place_worktrees(layout, &dest, &bare, &mut actions)?;
    if worktrees(&bare)?.is_empty()
        && let Some(path) = add_default(&dest, &bare)?
    {
        actions.push(format!("added {}", path.display()));
    }

    let mut text: String = actions.iter().map(|a| format!("{a}\n")).collect();
    for s in &skipped {
        eprintln!("forest: {s}");
    }
    text.push_str(&format!("{}\n", dest.display()));
    let mut out = Output::new(
        text,
        json!({"project": name, "path": dest, "actions": actions, "skipped": skipped}),
    );
    if !skipped.is_empty() {
        out.code = 1;
    }
    Ok(out)
}

/// What kind of repo `dir` holds, and its git dir.
fn inspect(dir: &Path) -> Result<(Kind, PathBuf)> {
    let bare = dir.join(BARE);
    if bare.is_dir() {
        return Ok((Kind::Layout, bare));
    }
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return Ok((Kind::DotGit { main: None }, dot_git));
    }
    if dot_git.is_file() {
        let common = git_out(
            dir,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .unwrap_or_else(|| "another repository".into());
        return Err(fail!(
            "{} is a linked worktree or submodule of {common}; migrate that repository instead",
            dir.display()
        ));
    }
    if git_out(dir, ["rev-parse", "--absolute-git-dir"])
        .is_some_and(|g| canonical(Path::new(&g)) == dir)
    {
        return Ok((Kind::Bare, dir.to_path_buf()));
    }
    Err(fail!("{} is not a git repository", dir.display()))
}

/// The branch of the main worktree, if it can be moved safely.
fn check_main(gitdir: &Path, dir: &Path, branch: &Option<String>) -> Result<String> {
    let shown = dir.display();
    let Some(branch) = branch else {
        return Err(Error::Refused(format!(
            "{shown} has a detached HEAD; check out a branch first"
        )));
    };
    if !ref_exists(gitdir, &format!("refs/heads/{branch}")) {
        return Err(fail!(
            "branch '{branch}' of {shown} has no commits yet; make a first commit"
        ));
    }
    if let Some(op) = IN_PROGRESS.iter().find(|f| gitdir.join(f).exists()) {
        return Err(Error::Refused(format!(
            "{shown} has an operation in progress ({op}); finish or abort it first"
        )));
    }
    if gitdir.join("modules").exists() {
        return Err(fail!(
            "{shown} has submodules, which migrate doesn't support"
        ));
    }
    for n in [BARE, STAGING] {
        if dir.join(n).exists() {
            return Err(fail!("{} is in the way", dir.join(n).display()));
        }
    }
    Ok(branch.clone())
}

/// Whether a worktree is where the layout wants it: in a workforest, or at its
/// branch dir in the project.
fn placed(layout: &Layout, dir: &Path, path: &Path, branch: Option<&str>) -> bool {
    path.starts_with(layout.root.join(FORESTS)) || branch.is_some_and(|b| path == dir.join(slug(b)))
}

/// Moves misplaced worktrees to their branch dir. Returns why some were left.
fn place_worktrees(
    layout: &Layout,
    dir: &Path,
    bare: &Path,
    actions: &mut Vec<String>,
) -> Result<Vec<String>> {
    // A move can free the dir another worktree needs, so repeat until stuck.
    loop {
        let mut list = worktrees(bare)?;
        list.retain(|w| !placed(layout, dir, &w.path, w.branch.as_deref()));
        // Nested worktrees first, so moving their parent doesn't strand them.
        list.sort_by_key(|w| std::cmp::Reverse(w.path.components().count()));
        let mut moved = false;
        let mut skipped = Vec::new();
        for w in &list {
            let shown = w.path.display();
            let Some(branch) = &w.branch else {
                skipped.push(format!("kept {shown}: detached HEAD"));
                continue;
            };
            let target = dir.join(slug(branch));
            if target.exists() {
                skipped.push(format!("kept {shown}: {} exists", target.display()));
                continue;
            }
            let args = [
                "worktree".as_ref(),
                "move".as_ref(),
                w.path.as_os_str(),
                target.as_os_str(),
            ];
            match git(bare, args) {
                Ok(_) => {
                    actions.push(format!("moved {shown} to {}", target.display()));
                    moved = true;
                }
                Err(e) => skipped.push(format!("kept {shown}: {e}")),
            }
        }
        if !moved {
            return Ok(skipped);
        }
    }
}

/// Adds the default-branch worktree, as `clone` would, when the branch exists.
fn add_default(dir: &Path, bare: &Path) -> Result<Option<PathBuf>> {
    let Ok(default) = default_branch(bare) else {
        return Ok(None);
    };
    let path = dir.join(slug(&default));
    let remote = format!("refs/remotes/origin/{default}");
    if ref_exists(bare, &format!("refs/heads/{default}")) {
        worktree_add(bare, &[], &path, &[&default])?;
    } else if ref_exists(bare, &remote) {
        worktree_add(bare, &["--track", "-b", &default], &path, &[&remote])?;
    } else {
        return Ok(None);
    }
    Ok(Some(path))
}

/// `<dir>/.git` when it is a file pointing at `<dir>/.bare`, a common hand-made
/// variant of the layout.
fn dot_git_link(dir: &Path) -> Option<PathBuf> {
    let file = dir.join(".git");
    let text = fs::read_to_string(&file).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    (canonical(&dir.join(target)) == canonical(&dir.join(BARE))).then_some(file)
}

/// Moves the entries of `from` that `select` picks into `to`.
fn move_entries(from: &Path, to: &Path, select: impl Fn(&str) -> bool) -> Result<()> {
    for entry in fs::read_dir(from)? {
        let name = entry?.file_name();
        if select(&name.to_string_lossy()) {
            fs::rename(from.join(&name), to.join(&name))?;
        }
    }
    Ok(())
}

/// `p` with its `from` prefix replaced by `to`.
fn rebase(p: &Path, from: &Path, to: &Path) -> PathBuf {
    match p.strip_prefix(from) {
        Ok(rest) => to.join(rest),
        Err(_) => p.to_path_buf(),
    }
}

/// `/src/shop` → `shop`, `/src/shop.git` → `shop`.
fn name_from_dir(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}
