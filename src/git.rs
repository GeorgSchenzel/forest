use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::error::{Result, fail};

fn command<I, S>(dir: &Path, args: I) -> (Command, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
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
