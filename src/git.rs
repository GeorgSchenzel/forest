use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::error::{Result, fail};

/// What forest's git invocations never run, wherever the repo's config
/// points: a box can write the main git dir its worktrees share, and
/// `core.fsmonitor` or a hook there would run as the host user the next
/// time forest (or anything else host-side) touches the repo. Set for
/// every invocation, in one place: here, where the Command is built.
const NO_HOST_CODE: [&str; 2] = ["core.fsmonitor=false", "core.hooksPath=/dev/null"];

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
}
