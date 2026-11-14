use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::error::{Result, fail};
use crate::layout::canonical;

/// What forest's git invocations never run, wherever the repo's config
/// points: a box can write the main git dir its worktrees share, and
/// `core.fsmonitor` or a hook there would run as the host user the next
/// time forest (or anything else host-side) touches the repo. Set for
/// every invocation, in one place: here, where the Command is built.
///
/// The last four close the config keys that name a program to run or a
/// transport to speak (B3): `core.sshCommand` and `diff.external` run
/// whatever they name, `protocol.file.allow=user` keeps a config-chosen
/// `file://` fetch from reaching host paths, and a pager is never spawned.
/// Filters cannot be closed that way (any `filter.<name>.clean` counts),
/// which is why git in a worktree gets its dirs pinned instead (see
/// [`WorktreeGit`]).
const NO_HOST_CODE: [&str; 6] = [
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "protocol.file.allow=user",
    "core.sshCommand=ssh",
    "diff.external=",
    "core.pager=cat",
];

fn command<I, S>(dir: &Path, args: I) -> (Command, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for cfg in NO_HOST_CODE {
        cmd.arg("-c").arg(cfg);
    }
    // The hardening flags are left out of the shown command: they are the
    // same for every invocation, so they only bury the part that varies.
    let mut shown = String::from("git");
    for a in args {
        shown.push(' ');
        shown.push_str(&a.as_ref().to_string_lossy());
        cmd.arg(a);
    }
    (cmd, shown)
}

/// Runs git in `dir` and returns its trimmed stdout, or an error carrying git's stderr.
pub fn git<I, S>(dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (mut cmd, shown) = command(dir, args);
    let out = cmd.output().map_err(|e| fail!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(fail!("{shown} failed: {}", stderr.trim()))
    }
}

/// Like `git`, but `None` on failure.
pub fn git_out<I, S>(dir: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git(dir, args).ok()
}

/// Runs git silently and reports whether it succeeded.
pub fn git_ok<I, S>(dir: &Path, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (mut cmd, _) = command(dir, args);
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Runs git with stderr shown to the user (progress, credential prompts); stdout is dropped
/// so it never mixes into forest's own output.
pub fn git_loud<I, S>(dir: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (mut cmd, shown) = command(dir, args);
    let status = cmd
        .stdout(Stdio::null())
        .status()
        .map_err(|e| fail!("cannot run git: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail!("{shown} failed"))
    }
}

/// Whether the installed git supports relative worktree paths (2.48+).
pub fn relative_paths() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let out = Command::new("git").arg("--version").output();
        let Ok(out) = out else { return false };
        let text = String::from_utf8_lossy(&out.stdout);
        // "git version 2.55.0" or "git version 2.39.3 (Apple Git-145)"
        let Some(version) = text.split_whitespace().nth(2) else {
            return false;
        };
        let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let major = parts.next().unwrap_or(0);
        let minor = parts.next().unwrap_or(0);
        (major, minor) >= (2, 48)
    })
}

/// Runs git and returns what it wrote to stderr (where e.g. `worktree prune -v` reports).
pub fn git_stderr<I, S>(dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (mut cmd, shown) = command(dir, args);
    let out = cmd.output().map_err(|e| fail!("cannot run git: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr).trim_end().to_string();
    if out.status.success() {
        Ok(stderr)
    } else {
        Err(fail!("{shown} failed: {stderr}"))
    }
}

// ---------------------------------------------------------------------------
// Linked worktrees: git dirs from the layout, not from box-writable files
// ---------------------------------------------------------------------------

/// The git dirs of a linked worktree, derived from the LAYOUT (B3): the
/// common dir is the repo's own `<root>/repos/<repo>/.git` — host-only, no
/// box can write it — and the worktree's git dir is the entry under
/// `<common>/worktrees/` whose `gitdir` file points at this worktree.
///
/// Git must never discover those through the worktree's `.git` file: a box
/// can rewrite it (or a `commondir` behind it) to point at a git dir whose
/// config chooses what runs on the host during `git status` — a filter
/// driver was reproduced doing exactly that. [`WorktreeGit::new`] refuses
/// anything that does not match the layout; [`git_in_worktree`] then runs
/// git with all three dirs pinned through the environment, so no
/// box-writable file chooses which config git reads.
#[derive(Debug, Clone)]
pub struct WorktreeGit {
    /// `<common>/worktrees/<id>` — git's `GIT_DIR` for this worktree.
    pub git_dir: PathBuf,
    /// `<root>/repos/<repo>/.git` — git's `GIT_COMMON_DIR`.
    pub common_dir: PathBuf,
    /// The worktree itself — git's `GIT_WORK_TREE`.
    pub work_tree: PathBuf,
}

impl WorktreeGit {
    /// Derives and validates the dirs for the worktree at `worktree` of the
    /// repo whose home checkout is `repo` (so the common dir is
    /// `repo/.git`). `Err` = refused: git must not run in this worktree,
    /// and the message says why (shown as the worktree's error row in `ls`).
    pub fn new(repo: &Path, worktree: &Path) -> Result<Self> {
        let common = repo.join(".git");
        if !common.is_dir() {
            return Err(fail!(
                "{} has no .git dir, so it is not a repo home; cannot run git in {}",
                repo.display(),
                worktree.display()
            ));
        }
        // The common dir itself must not redirect: a commondir file here
        // would send every git call, env-pinned or not, to another config.
        if common.join("commondir").exists() {
            return Err(fail!(
                "refusing {}: {}/commondir redirects the common dir",
                worktree.display(),
                common.display()
            ));
        }
        let work_tree = canonical(worktree);
        let entry = Self::find_entry(&common, &work_tree)?;
        let git_dir = canonical(&entry);
        // The worktree's `.git` file is the one box-writable link in this
        // chain: whatever it says, git may only ever reach the entry the
        // common dir already names.
        let pointed = read_gitfile(&work_tree.join(".git"))
            .map_err(|e| fail!("refusing {}: {e}", work_tree.display()))?;
        if canonical(&pointed) != git_dir {
            return Err(fail!(
                "refusing {}: its .git points at {}, but the repo's worktrees/ \
                 says {} (a box rewrote the pointer?)",
                work_tree.display(),
                pointed.display(),
                git_dir.display()
            ));
        }
        // The entry's commondir must be the plain parent chain; git ignores
        // it once GIT_COMMON_DIR is pinned, but a rewritten one means
        // someone redirected the repo side too.
        let commondir = std::fs::read_to_string(entry.join("commondir"))
            .map_err(|e| fail!("refusing {}: no commondir in {}: {e}", work_tree.display(), entry.display()))?;
        if commondir.trim() != "../.." {
            return Err(fail!(
                "refusing {}: {}/commondir is '{}' (want '../..')",
                work_tree.display(),
                entry.display(),
                commondir.trim()
            ));
        }
        Ok(WorktreeGit {
            git_dir,
            common_dir: canonical(&common),
            work_tree,
        })
    }

    /// The entry under `<common>/worktrees/` whose `gitdir` file names this
    /// worktree. Only host-side files are read: the entry, not the
    /// worktree, is the authority.
    fn find_entry(common: &Path, work_tree: &Path) -> Result<PathBuf> {
        let dir = common.join("worktrees");
        let entries = std::fs::read_dir(&dir).map_err(|e| fail!("reading {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path.join("gitdir")) else {
                continue; // a half-registered worktree; git would prune it
            };
            if let Some(named) = worktree_of_gitdir(&text, common, &path)
                && named == work_tree
            {
                return Ok(path);
            }
        }
        Err(fail!(
            "refusing {}: no entry under {} points at it (unregistered worktree?)",
            work_tree.display(),
            dir.display()
        ))
    }
}

/// The worktree a `gitdir` file's content names (it always names the
/// worktree's `.git`, absolute, or relative to the entry — git writes both
/// forms, `worktree.useRelativePaths`). None when it names no worktree.
fn worktree_of_gitdir(text: &str, common: &Path, entry: &Path) -> Option<PathBuf> {
    let target = Path::new(text.trim());
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        // both bases are host-side; taking the first that resolves keeps
        // this working across git versions' relative-path conventions
        let from_entry = canonical(&entry.join(target));
        if from_entry.is_dir() || from_entry.exists() {
            from_entry
        } else {
            canonical(&common.join(target))
        }
    };
    // the content names `<worktree>/.git`; the worktree is its parent
    (absolute.file_name()? == OsStr::new(".git"))
        .then(|| absolute.parent().map(canonical))?
}

/// The absolute dir a worktree's `.git` file points at (a `gitdir:` line,
/// relative to the file's own dir, as git resolves it). `Err` carries the
/// reason as plain text.
fn read_gitfile(path: &Path) -> std::result::Result<PathBuf, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("gitdir:"))
        .ok_or_else(|| format!("{} is not a gitfile (no gitdir: line)", path.display()))?;
    let target = Path::new(line.trim());
    Ok(if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new("")).join(target)
    })
}

/// Runs git in a linked worktree with the dirs pinned to the layout (B3):
/// `GIT_DIR`/`GIT_COMMON_DIR`/`GIT_WORK_TREE` come from [`WorktreeGit`],
/// all derived from host-side files, so no box-writable pointer can choose
/// the config — and so the host code — that git runs with. Same `-c`
/// hardening as every other invocation.
pub fn git_in_worktree<I, S>(dirs: &WorktreeGit, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (mut cmd, shown) = wt_command(dirs, args);
    let out = cmd.output().map_err(|e| fail!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(fail!("{shown} failed: {}", stderr.trim()))
    }
}

/// Like [`git_in_worktree`], but `None` on failure.
pub fn git_in_worktree_out<I, S>(dirs: &WorktreeGit, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_in_worktree(dirs, args).ok()
}

fn wt_command<I, S>(dirs: &WorktreeGit, args: I) -> (Command, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    for cfg in NO_HOST_CODE {
        cmd.arg("-c").arg(cfg);
    }
    cmd.env("GIT_DIR", &dirs.git_dir)
        .env("GIT_COMMON_DIR", &dirs.common_dir)
        .env("GIT_WORK_TREE", &dirs.work_tree);
    let mut shown = String::from("git (dirs pinned)");
    for a in args {
        shown.push(' ');
        shown.push_str(&a.as_ref().to_string_lossy());
        cmd.arg(a);
    }
    (cmd, shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A real repo, hermetic against the host's git config.
    fn repo(name: &str) -> (PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("forest-git-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("gitconfig"),
            "[user]\nname = T\nemail = t@example.com\n[init]\ndefaultBranch = main\n",
        )
        .unwrap();
        let dir = base.join("repo");
        fs::create_dir_all(&dir).unwrap();
        plain_git(&base, &dir, &["init", "-q"]);
        plain_git(
            &base,
            &dir,
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        (base, dir)
    }

    /// Git as the user would run it: no hardening flags (the control group
    /// the canary is proven with).
    fn plain_git(base: &Path, dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("git");
        let out = cmd
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", base.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("running git");
        assert!(
            out.status.success(),
            "plain git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// A hook script that touches a canary; the canary is gone until run.
    /// The mark is absolute: git runs hooks from the worktree top with a
    /// relative `$0`.
    fn canary_hook(base: &Path, name: &str) -> PathBuf {
        let hook = base.join(format!("{name}.sh"));
        let mark = base.join(name);
        let _ = fs::remove_file(&mark);
        fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", mark.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
        hook
    }

    fn ran(base: &Path, name: &str) -> bool {
        base.join(name).exists()
    }

    /// A repo whose `core.fsmonitor` touches a canary: forest's git must not
    /// run it, not even for `git status` — the command that consults the
    /// fsmonitor and runs on every `forest ls`.
    #[test]
    fn forest_git_never_runs_the_repo_fsmonitor() {
        let (base, dir) = repo("fsmonitor");
        let hook = canary_hook(&base, "fsmonitor-ran");
        plain_git(
            &base,
            &dir,
            &["config", "core.fsmonitor", &hook.display().to_string()],
        );
        // the canary works: plain git, without forest's hardening, runs it
        plain_git(&base, &dir, &["status", "--porcelain"]);
        assert!(
            ran(&base, "fsmonitor-ran"),
            "plain git must run the fsmonitor"
        );

        // forest's runner does not, for the commands that would consult it
        for args in [
            vec!["status", "--porcelain"],
            vec!["status", "--porcelain=v2", "--branch"],
            vec!["worktree", "list", "--porcelain"],
            vec![
                "worktree",
                "add",
                "--no-track",
                "-b",
                "x",
                &base.join("wt").display().to_string(),
            ],
        ] {
            let _ = fs::remove_file(base.join("fsmonitor-ran"));
            git(&dir, &args).expect("git with hardening runs");
            assert!(
                !ran(&base, "fsmonitor-ran"),
                "forest git ran the fsmonitor during {args:?}"
            );
        }
        fs::remove_dir_all(&base).unwrap();
    }

    /// A repo with a post-checkout hook: `git worktree add` — what `forest
    /// new` and `forest add` run — must not run it.
    #[test]
    fn forest_git_never_runs_repo_hooks() {
        let (base, dir) = repo("hooks");
        canary_hook(&base, "hook-ran");
        let hooks = dir.join(".git/hooks");
        fs::copy(base.join("hook-ran.sh"), hooks.join("post-checkout")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(
                hooks.join("post-checkout"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        // the canary works: plain worktree add runs the hook
        plain_git(
            &base,
            &dir,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "plain",
                &base.join("wt-plain").display().to_string(),
            ],
        );
        assert!(
            ran(&base, "hook-ran"),
            "plain git must run the post-checkout hook"
        );

        let _ = fs::remove_file(base.join("hook-ran"));
        git(
            &dir,
            [
                "worktree",
                "add",
                "--no-track",
                "-b",
                "hardened",
                &base.join("wt-hard").display().to_string(),
            ],
        )
        .expect("worktree add with hardening runs");
        assert!(!ran(&base, "hook-ran"), "forest git ran post-checkout");
        fs::remove_dir_all(&base).unwrap();
    }

    /// The hardening is in the one place every runner goes through: the
    /// Command builder. Checked by behavior above; this pins the flags
    /// themselves, so a rename cannot slip one past review.
    #[test]
    fn every_runner_carries_the_hardening_flags() {
        let (base, dir) = repo("flags");
        let (cmd, _shown) = command(&dir, ["status", "--porcelain"]);
        let argv: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let expected: Vec<String> = std::iter::once("-C".to_string())
            .chain([dir.display().to_string()])
            .chain(
                NO_HOST_CODE
                    .iter()
                    .flat_map(|c| ["-c".to_string(), (*c).to_string()]),
            )
            .chain(["status".to_string(), "--porcelain".to_string()])
            .collect();
        assert_eq!(argv, expected);
        fs::remove_dir_all(&base).unwrap();
    }

    // --- B3: git dirs from the layout, never from box-writable pointers ---

    /// A recursive copy for building fake git dirs from real ones.
    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// A repo with one linked worktree `wt` at `<base>/wt`, a tracked
    /// `f.txt` whose content changes without changing size (that forces a
    /// `git status` to hash, and so filter, the file), and a canary filter
    /// script that is registered in NO config the real repo has.
    fn worktree_repo(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let (base, dir) = repo(name);
        fs::write(dir.join("f.txt"), "aaaaaaaaa\n").unwrap();
        plain_git(&base, &dir, &["add", "."]);
        plain_git(&base, &dir, &["commit", "-q", "-m", "file"]);
        plain_git(
            &base,
            &dir,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "wt",
                &base.join("wt").display().to_string(),
            ],
        );
        let wt = base.join("wt");
        fs::write(wt.join(".gitattributes"), "f.txt filter=x\n").unwrap();
        plain_git(&base, &wt, &["add", ".gitattributes"]);
        plain_git(&base, &wt, &["commit", "-q", "-m", "attrs"]);
        fs::write(wt.join("f.txt"), "bbbbbbbbb\n").unwrap();
        (base, dir, wt)
    }

    /// The reviewer's B3 repro, both shapes: a box rewrites the worktree's
    /// `.git` (or the entry's `commondir`) to a config that runs a filter
    /// during a mere `git status`. Plain git runs the canary; forest
    /// refuses the worktree and never runs git in it.
    #[test]
    fn worktree_git_never_follows_box_writable_pointers() {
        let (base, dir, wt) = worktree_repo("b3");
        let filter = canary_hook(&base, "filter-ran");
        // the box-chosen common dir: a copy of the repo's git dir plus the
        // filter config
        let attacker = base.join("attacker");
        copy_dir(&dir.join(".git"), &attacker);
        let _ = fs::remove_dir_all(attacker.join("worktrees"));
        let mut cfg = fs::read_to_string(attacker.join("config")).unwrap();
        cfg.push_str(&format!(
            "[filter \"x\"]\n\tclean = {}\n\tsmudge = cat\n",
            filter.display()
        ));
        fs::write(attacker.join("config"), cfg).unwrap();
        // the fake entry the rewritten `.git` will point at
        let entry = dir.join(".git/worktrees/wt");
        let fake = base.join("fake-entry");
        fs::create_dir_all(&fake).unwrap();
        fs::write(fake.join("commondir"), format!("{}\n", attacker.display())).unwrap();
        fs::copy(entry.join("HEAD"), fake.join("HEAD")).unwrap();
        fs::copy(entry.join("index"), fake.join("index")).unwrap();

        // Shape 1: the worktree's `.git` points elsewhere. The control
        // first: plain git status runs the box-chosen filter.
        let gitfile = wt.join(".git");
        let real = fs::read_to_string(&gitfile).unwrap();
        fs::write(&gitfile, format!("gitdir: {}\n", fake.display())).unwrap();
        plain_git(&base, &wt, &["status", "--porcelain"]);
        assert!(
            ran(&base, "filter-ran"),
            "plain git must run the box-chosen filter"
        );
        let _ = fs::remove_file(base.join("filter-ran"));
        let err = WorktreeGit::new(&dir, &wt).unwrap_err();
        assert!(
            err.to_string().contains("points at"),
            "the refusal must say what happened: {err}"
        );
        assert!(!ran(&base, "filter-ran"), "forest ran git in a refused worktree");

        // Shape 2: the real entry's commondir redirects (host-side tamper,
        // same effect). Control first again.
        fs::write(&gitfile, &real).unwrap();
        fs::write(entry.join("commondir"), format!("{}\n", attacker.display())).unwrap();
        plain_git(&base, &wt, &["status", "--porcelain"]);
        assert!(ran(&base, "filter-ran"));
        let _ = fs::remove_file(base.join("filter-ran"));
        let err = WorktreeGit::new(&dir, &wt).unwrap_err();
        assert!(
            err.to_string().contains("commondir"),
            "the refusal must say what happened: {err}"
        );
        assert!(!ran(&base, "filter-ran"));

        // Restored, the worktree validates again and the pinned runner
        // reports the change without the filter ever being registered.
        fs::write(entry.join("commondir"), "../..\n").unwrap();
        let dirs = WorktreeGit::new(&dir, &wt).expect("a healthy worktree validates");
        assert_eq!(
            git_in_worktree(&dirs, ["status", "--porcelain"]).unwrap(),
            " M f.txt"
        );
        assert!(!ran(&base, "filter-ran"));
        fs::remove_dir_all(&base).unwrap();
    }

    /// The pinned runner does the worktree's own work: branch, config and
    /// rename go through the layout's dirs, for absolute and relative
    /// (git 2.48+) gitdir entries alike.
    #[test]
    fn the_pinned_runner_works_on_a_healthy_worktree() {
        let (base, dir) = repo("pin");
        plain_git(
            &base,
            &dir,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "wt",
                &base.join("wt").display().to_string(),
            ],
        );
        let wt = base.join("wt");
        let dirs = WorktreeGit::new(&dir, &wt).expect("a healthy worktree validates");
        assert_eq!(dirs.common_dir, canonical(&dir.join(".git")));
        assert_eq!(dirs.work_tree, canonical(&wt));
        assert_eq!(
            git_in_worktree(&dirs, ["branch", "--show-current"]).unwrap(),
            "wt"
        );
        assert_eq!(
            git_in_worktree(&dirs, ["status", "--porcelain"]).unwrap(),
            ""
        );
        // rename through the pinned dirs, the `name` command's one write
        git_in_worktree(&dirs, ["branch", "-m", "wt", "wt2"]).expect("branch -m");
        assert_eq!(
            git_in_worktree(&dirs, ["branch", "--show-current"]).unwrap(),
            "wt2"
        );
        let home = plain_git(&base, &dir, &["branch", "--show-current"]);
        assert_eq!(
            String::from_utf8_lossy(&home.stdout).trim(),
            "main",
            "the rename must not leak into the repo's home"
        );

        // relative-path entries (worktree.useRelativePaths) validate too
        if relative_paths() {
            plain_git(
                &base,
                &dir,
                &[
                    "worktree",
                    "add",
                    "--relative-paths",
                    "--no-track",
                    "-b",
                    "rel",
                    &base.join("rel").display().to_string(),
                ],
            );
            WorktreeGit::new(&dir, &base.join("rel"))
                .expect("a relative-path entry validates");
        }
        fs::remove_dir_all(&base).unwrap();
    }

    /// A common dir that itself redirects (a commondir file in `<repo>/.git`)
    /// is refused for every worktree of the repo.
    #[test]
    fn a_common_dir_that_redirects_is_refused() {
        let (base, dir) = repo("cd");
        plain_git(
            &base,
            &dir,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "wt",
                &base.join("wt").display().to_string(),
            ],
        );
        fs::write(dir.join(".git/commondir"), "../elsewhere\n").unwrap();
        let err = WorktreeGit::new(&dir, &base.join("wt")).unwrap_err();
        assert!(
            err.to_string().contains("redirects"),
            "the refusal must say what happened: {err}"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    /// The worktree runner pins the three dirs through the environment and
    /// carries the same `-c` hardening as every other invocation; pinned by
    /// argv/env inspection so a rename cannot slip one past review.
    #[test]
    fn the_worktree_runner_pins_dirs_and_carries_the_flags() {
        let (base, dir) = repo("wtenv");
        plain_git(
            &base,
            &dir,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "wt",
                &base.join("wt").display().to_string(),
            ],
        );
        let dirs = WorktreeGit::new(&dir, &base.join("wt")).unwrap();
        let (cmd, _) = wt_command(&dirs, ["status", "--porcelain"]);
        let mut flags: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for cfg in NO_HOST_CODE {
            assert_eq!(flags.remove(0), "-c");
            assert_eq!(flags.remove(0), cfg);
        }
        assert_eq!(flags, ["status", "--porcelain"]); // …then the command
        let env: std::collections::BTreeMap<_, _> = cmd.get_envs().collect();
        for (key, want) in [
            ("GIT_DIR", &dirs.git_dir),
            ("GIT_COMMON_DIR", &dirs.common_dir),
            ("GIT_WORK_TREE", &dirs.work_tree),
        ] {
            assert_eq!(
                env.get(OsStr::new(key)).copied().flatten(),
                Some(want.as_os_str()),
                "{key} must be pinned"
            );
        }
        fs::remove_dir_all(&base).unwrap();
    }
}
