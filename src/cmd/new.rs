//! `new`: put worktrees on a branch into its forest, making the forest if needed.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::Output;
use crate::cmd::path::Identity;
use crate::error::{Result, fail};
use crate::git::git;
use crate::layout::{Layout, slug, valid_name};
use crate::project::{
    Worktree, all_worktrees, check_branch_name, commit_exists, default_branch, fetch, forests_on,
    live_worktrees, origin_url, ref_exists, worktree_add,
};

/// Where a new worktree's branch came from.
enum Start {
    /// An existing local branch.
    Local,
    /// Created to track this remote branch.
    Remote(String),
    /// Created from this ref.
    New(String),
}

impl Start {
    fn created(&self) -> bool {
        !matches!(self, Start::Local)
    }

    fn from(&self) -> Option<&str> {
        match self {
            Start::Local => None,
            Start::Remote(r) | Start::New(r) => Some(r),
        }
    }

    fn report(&self, project: &str, branch: &str) {
        match self {
            Start::Local => eprintln!("{project}: checked out branch {branch}"),
            Start::Remote(r) => eprintln!("{project}: created branch {branch} tracking {r}"),
            Start::New(r) => eprintln!("{project}: created branch {branch} from {r}"),
        }
    }

    fn json(&self, project: &str, path: &Path) -> Value {
        json!({
            "project": project,
            "path": path,
            "branch_created": self.created(),
            "from": self.from(),
        })
    }
}

/// Puts a worktree of each project on `branch` into the forest that already has
/// that branch, else into a new forest named `alias` or after the branch. With
/// no branch (`--unnamed`), a generated wip branch is the forest's name too.
pub fn new(
    layout: &Layout,
    branch: Option<&str>,
    projects: &[String],
    alias: Option<&str>,
    from: Option<&str>,
    no_fetch: bool,
) -> Result<Output> {
    if let Some(b) = branch {
        check_branch_name(b)?;
    }
    let mut unique: Vec<&String> = Vec::new();
    for p in projects {
        check_project(layout, p)?;
        if !unique.contains(&p) {
            unique.push(p);
        }
    }

    let all = all_worktrees(layout)?;
    let branch = match branch {
        Some(b) => b.to_string(),
        None => wip_name(layout, &all, &unique)?,
    };
    let holding = forests_on(&all, layout, &branch);
    let (name, created) = match holding.as_slice() {
        [f] => {
            if let Some(a) = alias.filter(|a| a != f) {
                return Err(fail!(
                    "'{branch}' is in forest '{f}' already; --as '{a}' only names a new forest"
                ));
            }
            (f.clone(), false)
        }
        [] => {
            let name = alias.map_or_else(|| slug(&branch), str::to_string);
            check_new_name(layout, &all, &name, None)?;
            (name, true)
        }
        _ => {
            return Err(fail!(
                "'{branch}' is in several forests: {}",
                holding.join(", ")
            ));
        }
    };
    let dir = layout.forest_dir(&name);
    for p in &unique {
        if dir.join(p).exists() {
            return Err(fail!("{} already exists", dir.join(p).display()));
        }
    }

    fs::create_dir_all(&dir)?;
    let mut made: Vec<(&str, PathBuf, Start)> = Vec::new();
    for p in &unique {
        let path = dir.join(p);
        match create_worktree(layout, p, &branch, &path, from, no_fetch) {
            Ok(start) => {
                start.report(p, &branch);
                made.push((p, path, start));
            }
            Err(e) => {
                for (p, path, start) in made.iter().rev() {
                    let repo = layout.repo_dir(p);
                    let _ = git(
                        &repo,
                        [
                            "worktree".as_ref(),
                            "remove".as_ref(),
                            "--force".as_ref(),
                            path.as_os_str(),
                        ],
                    );
                    if start.created() {
                        let _ = git(&repo, ["branch", "-D", branch.as_str()]);
                    }
                }
                if created {
                    let _ = fs::remove_dir_all(&dir);
                }
                return Err(fail!("{p}: {e}\nnothing was created"));
            }
        }
    }
    let worktrees: Vec<_> = made
        .iter()
        .map(|(p, path, start)| start.json(p, path))
        .collect();
    Ok(Output::new(
        format!("{}\n", dir.display()),
        Identity::forest(&name, &dir, Some(branch.clone()))
            .with(json!({"created": created, "worktrees": worktrees})),
    ))
}

/// A new forest's name must be free, and must not name anything else a target can;
/// `own` is the forest being renamed, whose branch it may take.
pub fn check_new_name(
    layout: &Layout,
    all: &[(String, Worktree)],
    name: &str,
    own: Option<&str>,
) -> Result<()> {
    valid_name(name)?;
    let dir = layout.forest_dir(name);
    if dir.exists() {
        return Err(fail!(
            "{} already exists; pick another name with --as",
            dir.display()
        ));
    }
    if layout.is_project(name) {
        return Err(fail!(
            "'{name}' is the name of a repo; pick another name with --as"
        ));
    }
    if let Some(f) = forests_on(all, layout, name)
        .iter()
        .find(|f| Some(f.as_str()) != own)
    {
        return Err(fail!(
            "'{name}' is the branch of forest '{f}'; pick another name with --as"
        ));
    }
    Ok(())
}

fn check_project(layout: &Layout, project: &str) -> Result<()> {
    if layout.is_project(project) {
        Ok(())
    } else {
        Err(fail!("no repo named '{project}'"))
    }
}

/// Makes `path` a new worktree of `project` on `branch`. `from` is only for a new
/// branch, and must exist. `no_fetch` skips `fetch origin`, for when something
/// else keeps the repo fresh (or nothing should wait on the network).
fn create_worktree(
    layout: &Layout,
    project: &str,
    branch: &str,
    path: &Path,
    from: Option<&str>,
    no_fetch: bool,
) -> Result<Start> {
    let repo = layout.repo_dir(project);
    // Pruned first, so a deleted worktree can be made again.
    let existing = live_worktrees(&repo)?;
    if path.exists() || existing.iter().any(|w| w.path == path) {
        return Err(fail!("{} already exists", path.display()));
    }
    if let Some(w) = existing
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        return Err(fail!(
            "branch '{branch}' is already checked out at {}",
            w.path.display()
        ));
    }

    if !no_fetch && origin_url(&repo).is_some() {
        fetch(&repo)?;
    }
    let remote = format!("origin/{branch}");
    let exists = if ref_exists(&repo, &format!("refs/heads/{branch}")) {
        Some(Start::Local)
    } else if ref_exists(&repo, &format!("refs/remotes/{remote}")) {
        Some(Start::Remote(remote))
    } else {
        None
    };
    if let Some(start) = exists {
        if from.is_some() {
            let place = match start {
                Start::Remote(_) => "on origin",
                _ => "locally",
            };
            return Err(fail!(
                "branch '{branch}' already exists {place}; --from is only for a new branch"
            ));
        }
        match &start {
            Start::Remote(r) => {
                let r = format!("refs/remotes/{r}");
                worktree_add(&repo, &["--track", "-b", branch], path, &[&r])?
            }
            _ => worktree_add(&repo, &[], path, &[branch])?,
        }
        return Ok(start);
    }
    let base = match from {
        Some(f) if commit_exists(&repo, f) => f.to_string(),
        Some(f) => return Err(fail!("ref '{f}' not found in {project}")),
        None => default_base(&repo, project)?,
    };
    worktree_add(&repo, &["--no-track", "-b", branch], path, &[&base])?;
    Ok(Start::New(short_ref(&base).to_string()))
}

/// `origin/main` for `refs/remotes/origin/main`, `main` for `refs/heads/main`.
fn short_ref(r: &str) -> &str {
    r.strip_prefix("refs/remotes/")
        .or_else(|| r.strip_prefix("refs/heads/"))
        .unwrap_or(r)
}

/// The default branch to start new branches from: `origin/<default>` when it exists
/// (just fetched, so freshest), else the local one.
fn default_base(repo: &Path, project: &str) -> Result<String> {
    let default = default_branch(repo)?;
    for r in [
        format!("refs/remotes/origin/{default}"),
        format!("refs/heads/{default}"),
    ] {
        if commit_exists(repo, &r) {
            return Ok(r);
        }
    }
    Err(fail!(
        "the default branch '{default}' of {project} has no commits yet; make a first commit"
    ))
}

/// A name for a forest that starts without one: `wip-<MMDD>-<4 [a-z0-9]>`, its
/// branch as much as its dir. A name that is taken already is retried a few
/// times; with 36^4 names a day, one is always free.
fn wip_name(
    layout: &Layout,
    all: &[(String, Worktree)],
    projects: &[&String],
) -> Result<String> {
    let (month, day) = month_day(now_days());
    for _ in 0..8 {
        let name = format!("wip-{month:02}{day:02}-{}", wip_suffix(4));
        let taken = layout.forest_dir(&name).is_dir()
            || !forests_on(all, layout, &name).is_empty()
            || projects.iter().any(|p| {
                let repo = layout.repo_dir(p);
                ref_exists(&repo, &format!("refs/heads/{name}"))
                    || ref_exists(&repo, &format!("refs/remotes/origin/{name}"))
            });
        if !taken {
            return Ok(name);
        }
    }
    Err(fail!("no free wip-… name in 8 tries; try again"))
}

/// Days since the epoch, UTC.
fn now_days() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_secs() / 86400) as i64)
}

/// Month and day in the civil calendar, UTC, from days since the epoch.
fn month_day(days: i64) -> (i64, i64) {
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (m, d)
}

/// Random chars for a wip name; the clock and pid when /dev/urandom is
/// unreadable, which a collision is retried anyway.
fn wip_suffix(n: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; n];
    if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        let mut x = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64))
        .rotate_left(32)
        ^ u64::from(std::process::id());
        for b in &mut bytes {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (x >> 33) as u8;
        }
    }
    bytes
        .iter()
        .map(|b| CHARS[usize::from(*b) % CHARS.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_and_day_from_epoch_days() {
        assert_eq!(month_day(0), (1, 1)); // 1970-01-01
        assert_eq!(month_day(19723), (1, 1)); // 2024-01-01
        assert_eq!(month_day(19783), (3, 1)); // 2024-03-01, after a leap February
        assert_eq!(month_day(20770), (11, 13)); // 2026-11-13
    }

    #[test]
    fn wip_suffix_is_four_plain_chars() {
        let s = wip_suffix(4);
        assert_eq!(s.len(), 4);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "{s}"
        );
    }
}
