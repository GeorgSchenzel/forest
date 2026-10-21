//! `migrate`: bring an existing repo into the layout.
//!
//! Accepts a normal clone, a bare repo, or a project of the older layout (a bare
//! repo in `.bare` with its worktrees next to it). The repo ends up as a normal
//! clone at `<root>/repos/<name>/`; for a bare repo, the worktree on the default
//! branch becomes that clone's checkout. Every other worktree moves to its forest,
//! `<root>/forests/<branch>/<name>/`, except worktrees nested inside the repo.
//! Working files are moved, never checked out again, so uncommitted, untracked and
//! ignored files survive.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Output;
use crate::error::{Error, Result, fail};
use crate::git::{git, git_out};
use crate::layout::{Layout, canonical, slug, valid_name};
use crate::project::{
    all_worktrees, configure, default_branch, forests_on, live_worktrees, origin_url, ref_exists,
    worktree_repair, worktrees,
};

/// The older layout's dir for the bare repo.
const BARE: &str = ".bare";

/// Per-worktree state of an operation in progress, kept in a worktree's admin
/// dir; it would be lost when that worktree becomes the repo's checkout.
const IN_PROGRESS: &[&str] = &[
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "rebase-merge",
    "rebase-apply",
];

enum Kind {
    /// `<dir>/.git` is a git dir.
    Clone,
    /// A bare repo: `<dir>/.bare` (the older layout), or `<dir>` itself.
    Bare(PathBuf),
}

pub fn migrate(layout: &Layout, path: &Path, name: Option<&str>) -> Result<Output> {
    let src = path
        .canonicalize()
        .map_err(|e| fail!("{}: {e}", path.display()))?;
    let name = match name {
        Some(n) => n.to_string(),
        None => name_from_dir(&src)
            .ok_or_else(|| fail!("cannot derive a repo name from {}; pass one", src.display()))?,
    };
    valid_name(&name)?;
    let dest = layout.repo_dir(&name);
    if layout.root.starts_with(&src)
        || src.starts_with(layout.forests_dir())
        || (src.starts_with(layout.repos_dir()) && src != dest)
    {
        return Err(fail!("{} can't be migrated", src.display()));
    }
    if dest != src && dest.exists() {
        return Err(fail!("{} already exists", dest.display()));
    }

    let kind = inspect(&src)?;
    let gitdir = match &kind {
        Kind::Clone => src.join(".git"),
        Kind::Bare(g) => g.clone(),
    };
    let list = live_worktrees(&gitdir)?;
    let mut actions = Vec::new();
    let mut linked: Vec<PathBuf> = list
        .iter()
        .filter(|w| !w.home)
        .map(|w| w.path.clone())
        .collect();

    match kind {
        Kind::Clone => {
            if dest != src {
                fs::create_dir_all(layout.repos_dir())?;
                fs::rename(&src, &dest).map_err(|e| {
                    fail!("cannot move {} to {}: {e}", src.display(), dest.display())
                })?;
                actions.push(format!("moved {} to {}", src.display(), dest.display()));
                for p in &mut linked {
                    *p = rebase(p, &src, &dest);
                }
            }
        }
        Kind::Bare(gitdir) => {
            let default = default_branch(&gitdir)?;
            let home = list
                .iter()
                .filter(|w| !w.home)
                .find(|w| w.branch.as_deref() == Some(default.as_str()))
                .map(|w| w.path.clone());
            let admin = match &home {
                Some(h) => Some(check_home(&gitdir, h)?),
                None => None,
            };
            if gitdir.join("modules").exists() {
                return Err(fail!(
                    "{} has submodules, which migrate doesn't support",
                    src.display()
                ));
            }

            fs::create_dir_all(&dest)?;
            let dot_git = dest.join(".git");
            if gitdir == src {
                // Worktrees made inside the bare repo stay where they are.
                fs::create_dir(&dot_git)?;
                let keep = linked.clone();
                move_entries(&src, &dot_git, |n| {
                    !keep.iter().any(|w| w.starts_with(src.join(n)))
                })?;
            } else {
                fs::rename(&gitdir, &dot_git)?;
                for p in &mut linked {
                    *p = rebase(p, &gitdir, &dot_git);
                }
            }
            actions.push(format!("moved the repo to {}", dot_git.display()));
            git(&dest, ["config", "core.bare", "false"])?;
            let _ = git_out(&dest, ["config", "--unset", "core.worktree"]);
            if origin_url(&dest).is_some()
                && git_out(&dest, ["config", "remote.origin.fetch"]).is_none()
            {
                // A `clone --bare` maps branches directly; track them as a clone would.
                git(
                    &dest,
                    [
                        "config",
                        "remote.origin.fetch",
                        "+refs/heads/*:refs/remotes/origin/*",
                    ],
                )?;
            }
            git(
                &dest,
                ["symbolic-ref", "HEAD", &format!("refs/heads/{default}")],
            )?;

            if let (Some(home), Some(admin)) = (home, admin) {
                // The default-branch worktree becomes the checkout: its files, and
                // its index, which keeps what was staged.
                move_entries(&home, &dest, |n| n != ".git")?;
                let admin = dot_git.join("worktrees").join(admin);
                if admin.join("index").exists() {
                    fs::rename(admin.join("index"), dot_git.join("index"))?;
                }
                fs::remove_dir_all(&admin)?;
                fs::remove_file(home.join(".git"))?;
                fs::remove_dir(&home)?;
                linked.retain(|p| p != &home);
                for p in &mut linked {
                    *p = rebase(p, &home, &dest);
                }
                actions.push(format!("moved {} to {}", home.display(), dest.display()));
            } else if ref_exists(&dest, &format!("refs/heads/{default}")) {
                git(&dest, ["reset", "--hard", "--quiet"])?;
                actions.push(format!("checked out {default} in {}", dest.display()));
            }
        }
    }

    if !linked.is_empty() {
        worktree_repair(&dest, &linked)?;
    }
    configure(&dest)?;
    if origin_url(&dest).is_some() && !ref_exists(&dest, "refs/remotes/origin/HEAD") {
        // Needs the network; without it the default falls back to our own HEAD.
        let _ = git_out(&dest, ["remote", "set-head", "origin", "--auto"]);
    }

    let skipped = place_worktrees(layout, &name, &dest, &mut actions)?;
    if src != dest && src.exists() {
        if fs::read_dir(&src).is_ok_and(|mut d| d.next().is_none()) {
            fs::remove_dir(&src)?;
        } else {
            eprintln!("forest: left {}: it is not empty", src.display());
        }
    }

    let mut text: String = actions.iter().map(|a| format!("{a}\n")).collect();
    if actions.is_empty() {
        text.push_str(&format!("{name} is already in the layout\n"));
    }
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

/// What kind of repo `dir` holds.
fn inspect(dir: &Path) -> Result<Kind> {
    let bare = dir.join(BARE);
    if bare.is_dir() {
        return Ok(Kind::Bare(bare));
    }
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return Ok(Kind::Clone);
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
        return Ok(Kind::Bare(dir.to_path_buf()));
    }
    Err(fail!("{} is not a git repository", dir.display()))
}

/// The admin dir name of the worktree that becomes the checkout, if that can be
/// done safely.
fn check_home(gitdir: &Path, home: &Path) -> Result<String> {
    let shown = home.display();
    let text = fs::read_to_string(home.join(".git"))
        .map_err(|e| fail!("cannot read {}: {e}", home.join(".git").display()))?;
    let admin = text
        .trim()
        .strip_prefix("gitdir:")
        .and_then(|p| Path::new(p.trim()).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| fail!("cannot read {}", home.join(".git").display()))?;
    let admin_dir = gitdir.join("worktrees").join(&admin);
    if let Some(op) = IN_PROGRESS.iter().find(|f| admin_dir.join(f).exists()) {
        return Err(Error::Refused(format!(
            "{shown} has an operation in progress ({op}); finish or abort it first"
        )));
    }
    if home.join(".git").is_dir() || home.join(BARE).exists() {
        return Err(fail!("{shown} can't become the checkout"));
    }
    Ok(admin)
}

/// Moves each worktree outside the repo dir to its forest. Returns why some were left.
fn place_worktrees(
    layout: &Layout,
    name: &str,
    dest: &Path,
    actions: &mut Vec<String>,
) -> Result<Vec<String>> {
    let mut skipped = Vec::new();
    for w in worktrees(dest)? {
        if w.home || w.path.starts_with(dest) || layout.forest_of(&w.path).is_some() {
            continue;
        }
        let shown = w.path.display();
        let Some(branch) = &w.branch else {
            skipped.push(format!("kept {shown}: detached HEAD"));
            continue;
        };
        let all = all_worktrees(layout)?;
        let forest = forests_on(&all, layout, branch)
            .into_iter()
            .next()
            .unwrap_or_else(|| slug(branch));
        let dir = layout.forest_dir(&forest);
        let target = dir.join(name);
        if target.exists() {
            skipped.push(format!("kept {shown}: {} exists", target.display()));
            continue;
        }
        fs::create_dir_all(&dir)?;
        let args = [
            "worktree".as_ref(),
            "move".as_ref(),
            w.path.as_os_str(),
            target.as_os_str(),
        ];
        match git(dest, args) {
            Ok(_) => actions.push(format!("moved {shown} to {}", target.display())),
            Err(e) => {
                let _ = fs::remove_dir(&dir);
                skipped.push(format!("kept {shown}: {e}"));
            }
        }
    }
    Ok(skipped)
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
