use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    base: PathBuf,
    root: PathBuf,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect("valid json")
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            base.join("gitconfig"),
            "[user]\nname = T\nemail = t@example.com\n[init]\ndefaultBranch = main\n",
        )
        .unwrap();
        Env {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn envs(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_GLOBAL", self.base.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("FOREST_ROOT", &self.root)
            .env("HOME", &self.base);
    }

    fn forest_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_forest"));
        cmd.current_dir(cwd).args(args);
        self.envs(&mut cmd);
        let out = cmd.output().unwrap();
        Run {
            code: out.status.code().unwrap(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn forest(&self, args: &[&str]) -> Run {
        self.forest_in(&self.base, args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let r = self.forest(args);
        assert_eq!(r.code, 0, "forest {args:?} failed: {}", r.stderr);
        r.stdout.trim_end().to_string()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args);
        self.envs(&mut cmd);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn commit(&self, dir: &Path, msg: &str) {
        self.git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    }

    /// A normal repo with one commit on main, to clone from.
    fn upstream(&self, name: &str) -> PathBuf {
        let dir = self.base.join("upstream").join(name);
        fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q"]);
        self.commit(&dir, "first");
        dir
    }

    /// `init` + first commit, so branches can be made.
    fn project(&self, name: &str) -> PathBuf {
        let wt = PathBuf::from(self.ok(&["init", name]));
        self.commit(&wt, "first");
        wt
    }
}

/// The forest dir of `name`.
fn forest_dir(e: &Env, name: &str) -> PathBuf {
    e.root.join("forests").join(name)
}

fn has_branch(e: &Env, repo: &Path, b: &str) -> bool {
    !e.git(repo, &["branch", "--list", b]).is_empty()
}

#[test]
fn init_creates_a_repo() {
    let e = Env::new();
    let wt = e.ok(&["init", "app", "--branch", "trunk"]);
    assert_eq!(PathBuf::from(&wt), e.root.join("repos/app"));
    assert!(e.root.join("repos/app/.git").is_dir());

    let ls = e.forest(&["ls", "--json"]).json();
    assert_eq!(ls["v"], 1);
    assert_eq!(ls["repos"][0]["name"], "app");
    assert_eq!(ls["worktrees"][0]["home"], true);
    assert_eq!(ls["worktrees"][0]["branch"], "trunk");
    assert_eq!(ls["worktrees"][0]["head"], Value::Null);

    let r = e.forest(&["new", "feat/x", "app"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("no commits yet"), "{}", r.stderr);
    assert!(!forest_dir(&e, "feat-x").exists());
}

#[test]
fn init_rejects_bad_names() {
    let e = Env::new();
    assert_eq!(e.forest(&["init", ".x"]).code, 1);
    assert_eq!(e.forest(&["init", "a:b"]).code, 1);
    e.ok(&["init", "app"]);
    assert_eq!(e.forest(&["init", "app"]).code, 1);
}

#[test]
fn clone_makes_a_normal_clone() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.git(&up, &["branch", "-m", "main", "develop"]);
    e.git(&up, &["branch", "feat/remote"]);
    let wt = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));
    assert_eq!(wt, e.root.join("repos/svc"));
    assert_eq!(e.git(&wt, &["branch", "--show-current"]), "develop");
    assert_eq!(
        e.git(&wt, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/develop"
    );
    // Only the default branch is local; the rest stays remote-tracking.
    assert_eq!(
        e.git(&wt, &["branch", "--format=%(refname:short)"]),
        "develop"
    );

    // Worktrees link back with relative paths.
    e.ok(&["new", "feat/remote", "svc"]);
    let gitfile = fs::read_to_string(forest_dir(&e, "feat-remote").join("svc/.git")).unwrap();
    assert!(gitfile.contains("../repos/svc/.git"), "{gitfile}");
}

#[test]
fn new_resolves_local_remote_and_new_branches() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.git(&up, &["branch", "feat/remote"]);
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));

    // Remote branch: tracking set up.
    let f = PathBuf::from(e.ok(&["new", "feat/remote", "svc"]));
    assert_eq!(f, forest_dir(&e, "feat-remote"));
    assert_eq!(
        e.git(
            &f.join("svc"),
            &["rev-parse", "--abbrev-ref", "@{upstream}"]
        ),
        "origin/feat/remote"
    );

    // New branch from the default branch, without an upstream.
    let wt = PathBuf::from(e.ok(&["new", "feat/new", "svc"])).join("svc");
    assert!(
        e.git(
            &wt,
            &[
                "for-each-ref",
                "--format=%(upstream)",
                "refs/heads/feat/new"
            ]
        )
        .is_empty()
    );

    // New branch from --from.
    e.commit(&wt, "on new");
    let wt2 = PathBuf::from(e.ok(&["new", "feat/stacked", "svc", "--from", "feat/new"]));
    assert_eq!(
        e.git(&wt2.join("svc"), &["log", "-1", "--format=%s"]),
        "on new"
    );

    // Local branch without a worktree.
    e.git(&repo, &["branch", "local-only", "feat/new"]);
    let wt3 = PathBuf::from(e.ok(&["new", "local-only", "svc"]));
    assert_eq!(
        e.git(&wt3.join("svc"), &["log", "-1", "--format=%s"]),
        "on new"
    );

    // An unknown --from is an error, and so is --from for a branch that exists.
    assert_eq!(e.forest(&["new", "x", "svc", "--from", "nope"]).code, 1);
    assert!(!forest_dir(&e, "x").exists());
    e.git(&repo, &["branch", "old", "feat/new"]);
    let r = e.forest(&["new", "old", "svc", "--from", "main"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("already exists locally"), "{}", r.stderr);
}

#[test]
fn new_reports_how_the_branch_was_set_up() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.git(&up, &["branch", "feat/remote"]);
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));
    e.git(&repo, &["branch", "local", "main"]);

    let cases = [
        (
            "feat/remote",
            "created branch feat/remote tracking origin/feat/remote",
            true,
            Value::from("origin/feat/remote"),
        ),
        (
            "feat/new",
            "created branch feat/new from origin/main",
            true,
            Value::from("origin/main"),
        ),
        ("local", "checked out branch local", false, Value::Null),
    ];
    for (branch, msg, created, from) in cases {
        let r = e.forest(&["new", branch, "svc", "--json"]);
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert!(r.stderr.contains(&format!("svc: {msg}")), "{}", r.stderr);
        let w = &r.json()["worktrees"][0];
        assert_eq!(w["branch_created"], created, "{branch}");
        assert_eq!(w["from"], from, "{branch}");
    }
}

#[test]
fn new_adds_to_the_forest_of_its_branch() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let f = forest_dir(&e, "feat-y");

    let r = e.forest(&["new", "feat/y", "api", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["created"], true);
    let r = e.forest(&["new", "feat/y", "web", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["created"], false);
    assert_eq!(r.json()["path"], f.to_str().unwrap());
    assert_eq!(
        e.git(&f.join("web"), &["branch", "--show-current"]),
        "feat/y"
    );

    // A worktree that is there already: nothing changes.
    let r = e.forest(&["new", "feat/y", "api"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("already exists"), "{}", r.stderr);
    // --as only names a new forest.
    let r = e.forest(&["new", "feat/y", "api", "--as", "other"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("in forest 'feat-y' already"),
        "{}",
        r.stderr
    );
}

#[test]
fn new_with_an_alias() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.project("db");
    let f = PathBuf::from(e.ok(&["new", "feat/auth", "api", "web", "--as", "tuna-auth"]));
    assert_eq!(f, forest_dir(&e, "tuna-auth"));
    assert_eq!(e.ok(&["path", "feat/auth"]), f.to_str().unwrap());
    assert_eq!(e.ok(&["path", "tuna-auth"]), f.to_str().unwrap());
    // Adding goes by branch.
    e.ok(&["new", "feat/auth", "db"]);
    assert!(f.join("db").is_dir());

    // A name must not be taken by a dir, a repo, or the branch of a forest.
    for (args, msg) in [
        (
            &["new", "b1", "api", "--as", "tuna-auth"][..],
            "already exists",
        ),
        (&["new", "b2", "api", "--as", "web"], "name of a repo"),
        (&["new", "b3", "api", "--as", "feat/auth"], "invalid name"),
        (&["new", "b4", "api", "--as", "x"], ""),
        (&["new", "b5", "web", "--as", "b4"], "branch of forest 'x'"),
        (&["new", "web", "api"], "name of a repo"),
    ] {
        let r = e.forest(args);
        if msg.is_empty() {
            assert_eq!(r.code, 0, "{args:?}: {}", r.stderr);
        } else {
            assert_eq!(r.code, 1, "{args:?}");
            assert!(r.stderr.contains(msg), "{args:?}: {}", r.stderr);
        }
    }
}

#[test]
fn new_is_all_or_nothing() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.ok(&["init", "empty"]); // unborn: can't branch
    let api = e.root.join("repos/api");
    let web = e.root.join("repos/web");
    e.git(&api, &["branch", "feat/old", "main"]);

    // One repo fails: the others are rolled back, but an existing branch stays.
    let r = e.forest(&["new", "feat/old", "web", "api", "empty"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("nothing was created"), "{}", r.stderr);
    assert!(!forest_dir(&e, "feat-old").exists());
    assert!(!has_branch(&e, &web, "feat/old"));
    assert!(has_branch(&e, &api, "feat/old"));
    assert!(!e.git(&api, &["worktree", "list"]).contains("forests"));

    // Adding to an existing forest fails the same way, and keeps the forest.
    let f = PathBuf::from(e.ok(&["new", "feat/z", "api"]));
    let r = e.forest(&["new", "feat/z", "web", "empty"]);
    assert_eq!(r.code, 1);
    assert!(f.join("api").is_dir() && !f.join("web").exists());

    // A --from missing in any repo fails the whole forest.
    e.git(&api, &["branch", "only-api", "main"]);
    let r = e.forest(&["new", "feat/w", "api", "web", "--from", "only-api"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("ref 'only-api' not found in web"),
        "{}",
        r.stderr
    );
    assert!(!forest_dir(&e, "feat-w").exists());
    assert!(!has_branch(&e, &api, "feat/w"));

    // A branch checked out in a repo's home can't go into a forest.
    let r = e.forest(&["new", "main", "api"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("already checked out at"), "{}", r.stderr);
    assert!(!forest_dir(&e, "main").exists());

    // Usage: at least one repo.
    assert_eq!(e.forest(&["new", "feat/q"]).code, 1);
}

#[test]
fn targets() {
    let e = Env::new();
    let api = e.project("api");
    e.project("web");
    let f = PathBuf::from(e.ok(&["new", "feat/y", "api", "web", "--as", "shop"]));
    let s = |p: &Path| p.to_str().unwrap().to_string();

    assert_eq!(e.ok(&["path", "shop"]), s(&f));
    assert_eq!(e.ok(&["path", "feat/y"]), s(&f));
    assert_eq!(e.ok(&["path", "shop", "api"]), s(&f.join("api")));
    assert_eq!(e.ok(&["path", "feat/y", "web"]), s(&f.join("web")));
    assert_eq!(e.ok(&["path", "api"]), s(&api));
    assert_eq!(e.ok(&["path", "main", "api"]), s(&api));

    let r = e.forest(&["path", "nope"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("no forest, branch or repo"),
        "{}",
        r.stderr
    );
    let r = e.forest(&["path", "shop", "nope"]);
    assert!(r.stderr.contains("no repo named 'nope'"), "{}", r.stderr);
    e.project("db");
    let r = e.forest(&["path", "shop", "db"]);
    assert!(r.stderr.contains("has no worktree of db"), "{}", r.stderr);

    // A word that names two forests is an error, never a guess.
    e.ok(&["new", "x", "db", "--as", "other"]);
    fs::create_dir_all(forest_dir(&e, "x")).unwrap();
    let r = e.forest(&["path", "x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("ambiguous"), "{}", r.stderr);

    // Without a target: the forest or repo cwd is in.
    let deep = f.join("api/deep");
    fs::create_dir_all(&deep).unwrap();
    assert_eq!(e.forest_in(&deep, &["path"]).stdout.trim(), s(&f));
    assert_eq!(e.forest_in(&f, &["path"]).stdout.trim(), s(&f));
    let sub = api.join("sub");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(e.forest_in(&sub, &["path"]).stdout.trim(), s(&api));
    let r = e.forest_in(&e.base, &["path"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("give a target"), "{}", r.stderr);
}

#[test]
fn path_steps_into_the_only_worktree() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let wt = PathBuf::from(e.ok(&["new", "feat/x", "api"])).join("api");
    let s = |p: &Path| p.to_str().unwrap().to_string();
    assert_eq!(e.ok(&["path", "feat/x"]), s(&wt));
    assert_eq!(e.ok(&["path", "feat-x"]), s(&wt));
    assert_eq!(e.forest_in(&wt, &["path"]).stdout.trim(), s(&wt));
    let j = e.forest(&["path", "feat/x", "--json"]).json();
    let want = identity(
        "worktree",
        "feat-x".into(),
        "api".into(),
        "feat/x".into(),
        &wt,
    );
    assert_eq!(j, want);

    // With a second repo, the forest is the forest again.
    e.ok(&["new", "feat/x", "web"]);
    assert_eq!(e.ok(&["path", "feat/x"]), s(&forest_dir(&e, "feat-x")));
    // And `rm` still means the whole forest.
    e.ok(&["rm", "feat/x", "web"]);
    e.ok(&["rm", "feat/x", "api"]);
    assert!(!forest_dir(&e, "feat-x").exists());
}

#[test]
fn pick_needs_a_terminal() {
    let e = Env::new();
    e.project("api");
    let r = e.forest(&["path", "--pick"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("needs a terminal"), "{}", r.stderr);
    let r = e.forest(&["path", "--pick", "api"]);
    assert_eq!(r.code, 1);
}

fn complete(e: &Env, words: &[&str]) -> Vec<String> {
    let mut args = vec!["__complete", "--"];
    args.extend(words);
    e.ok(&args).lines().map(String::from).collect()
}

#[test]
fn completion() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.ok(&["new", "feat/y", "api", "web", "--as", "shop"]);
    e.ok(&["new", "feat/x", "api"]);

    let subs = complete(&e, &["forest", ""]);
    assert!(subs.contains(&"path".into()) && subs.contains(&"shell-init".into()));
    assert!(!subs.iter().any(|s| s.starts_with("__")), "{subs:?}");

    let targets = ["api", "feat-x", "feat/x", "feat/y", "shop", "web"];
    assert_eq!(complete(&e, &["forest", "path", ""]), targets);
    assert_eq!(complete(&e, &["forest", "--json", "rm", "f"]), targets);
    assert_eq!(complete(&e, &["forest", "done", ""]), targets);
    assert_eq!(complete(&e, &["forest", "push", "f"]), targets);
    assert_eq!(complete(&e, &["fcd", ""]), targets);
    // The second word is a repo of that forest, or any repo.
    assert_eq!(complete(&e, &["fcd", "feat/x", ""]), ["api"]);
    assert_eq!(
        complete(&e, &["forest", "path", "shop", ""]),
        ["api", "web"]
    );
    assert_eq!(complete(&e, &["forest", "done", "feat/x", ""]), ["api"]);
    assert_eq!(
        complete(&e, &["forest", "push", "shop", ""]),
        ["api", "web"]
    );
    assert_eq!(
        complete(&e, &["forest", "path", "main", ""]),
        ["api", "web"]
    );
    assert_eq!(
        complete(&e, &["forest", "path", "shop", "api", ""]),
        [] as [&str; 0]
    );

    assert_eq!(complete(&e, &["forest", "mv", ""]), ["feat-x", "shop"]);
    assert_eq!(complete(&e, &["forest", "new", ""]), ["feat/x", "feat/y"]);
    assert_eq!(complete(&e, &["forest", "new", "b", "api", ""]), ["web"]);
    // A flag's value is skipped, and isn't a positional.
    assert_eq!(
        complete(&e, &["forest", "new", "--from", "main", "b", ""]),
        ["api", "web"]
    );
    assert_eq!(
        complete(&e, &["forest", "new", "b", "--from", ""]),
        [] as [&str; 0]
    );
    assert_eq!(complete(&e, &["forest", "--root", ""]), [":files"]);
    assert_eq!(complete(&e, &["forest", "migrate", ""]), [":files"]);

    let flags = complete(&e, &["forest", "rm", "-"]);
    assert!(flags.contains(&"--delete-branch".into()) && flags.contains(&"--json".into()));

    // `--root` on the line being completed is the root to look in.
    let other = e.base.join("other");
    fs::create_dir_all(&other).unwrap();
    let root = other.to_str().unwrap();
    assert_eq!(
        complete(&e, &["forest", "--root", root, "path", ""]),
        [] as [&str; 0]
    );
}

#[test]
fn shell_init_defines_fcd_and_completion() {
    let e = Env::new();
    let out = e.ok(&["shell-init", "zsh"]);
    assert!(out.contains("fcd()"), "{out}");
    assert!(out.contains("compdef _forest forest fcd"), "{out}");
    assert!(out.contains("add-zsh-hook precmd _forest_compdef"), "{out}");
}

#[test]
fn rm_refuses_dirty_worktrees() {
    let e = Env::new();
    e.project("app");
    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    let x = f.join("app");
    fs::write(x.join("new.txt"), "x").unwrap();

    let r = e.forest(&["rm", "feat/x", "app", "--json"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["code"], "refused");
    assert!(x.exists());

    // Ignored files don't count.
    fs::remove_file(x.join("new.txt")).unwrap();
    fs::write(x.join(".gitignore"), "*.log\n").unwrap();
    e.git(&x, &["add", ".gitignore"]);
    e.commit(&x, "ignore");
    fs::write(x.join("a.log"), "x").unwrap();
    e.ok(&["rm", "feat/x", "app"]);
    assert!(!f.exists());
    // The branch is kept.
    assert!(has_branch(&e, &e.root.join("repos/app"), "feat/x"));
}

#[test]
fn rm_one_worktree_of_a_forest() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let f = PathBuf::from(e.ok(&["new", "feat/y", "api", "web"]));

    let r = e.forest(&["rm", "feat/y", "api", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["kind"], "worktree");
    assert_eq!(j["forest"], "feat-y");
    assert!(!f.join("api").exists() && f.join("web").exists());

    // The forest goes with its last worktree.
    let out = e.ok(&["rm", "feat-y", "web"]);
    assert!(out.ends_with(&format!("removed {}", f.display())), "{out}");
    assert!(!f.exists());
}

#[test]
fn rm_never_removes_a_home() {
    let e = Env::new();
    let app = e.project("app");
    for args in [&["rm", "app"][..], &["rm", "main", "app", "--force"]] {
        let r = e.forest(args);
        assert_eq!(r.code, 1, "{args:?}");
        assert!(r.stderr.contains("is the home of app"), "{}", r.stderr);
    }
    let r = e.forest_in(&app, &["rm"]);
    assert_eq!(r.code, 1);
    assert!(app.join(".git").is_dir());
}

#[test]
fn rm_delete_branch_never_loses_commits() {
    let e = Env::new();
    let up = e.upstream("svc");
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));

    // Unpushed commits: refused.
    let a = PathBuf::from(e.ok(&["new", "feat/a", "svc"])).join("svc");
    e.commit(&a, "unpushed");
    assert_eq!(
        e.forest(&["rm", "feat/a", "svc", "--delete-branch"]).code,
        2
    );
    assert!(a.exists());

    // Nothing beyond main: deleted.
    e.ok(&["new", "feat/b", "svc"]);
    e.ok(&["rm", "feat/b", "svc", "--delete-branch"]);
    assert!(!has_branch(&e, &repo, "feat/b"));

    // Upstream gone, but with a commit that was never pushed: refused.
    e.git(&up, &["branch", "feat/c"]);
    let c = PathBuf::from(e.ok(&["new", "feat/c", "svc"])).join("svc");
    e.commit(&c, "never pushed");
    e.git(&up, &["branch", "-D", "feat/c"]);
    e.git(&repo, &["fetch", "-q", "--prune"]);
    assert_eq!(
        e.forest(&["rm", "feat/c", "svc", "--delete-branch"]).code,
        2
    );
    // Upstream gone after a real merge: everything is on main, so deleted.
    e.git(&up, &["branch", "feat/d"]);
    e.ok(&["new", "feat/d", "svc"]);
    e.git(&up, &["branch", "-D", "feat/d"]);
    e.git(&repo, &["fetch", "-q", "--prune"]);
    e.ok(&["rm", "feat/d", "svc", "--delete-branch"]);
    assert!(!has_branch(&e, &repo, "feat/d"));

    // --force overrides, except for the default branch.
    e.ok(&["rm", "feat/a", "svc", "--delete-branch", "--force"]);
    assert!(!has_branch(&e, &repo, "feat/a"));
    e.git(&repo, &["checkout", "-q", "-b", "elsewhere"]);
    e.ok(&["new", "main", "svc"]);
    let r = e.forest(&["rm", "main", "svc", "--delete-branch", "--force"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("never deleted"), "{}", r.stderr);
}

#[test]
fn rm_finds_missing_worktrees_and_keeps_strays() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let f = PathBuf::from(e.ok(&["new", "feat/y", "api", "web", "--as", "shop"]));
    fs::remove_dir_all(f.join("web")).unwrap();
    fs::create_dir_all(f.join("notes")).unwrap();
    fs::write(f.join("notes/todo"), "x").unwrap();

    // The stray dir refuses without --force.
    assert_eq!(e.forest_in(&f, &["rm"]).code, 2);

    // With --force, both worktrees go, but the stray dir is kept.
    let r = e.forest_in(&f, &["rm", "--force", "--delete-branch", "--json"]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["removed"].as_array().unwrap().len(), 2);
    assert_eq!(j["errors"].as_array().unwrap().len(), 1);
    assert!(f.join("notes/todo").exists());
    assert!(!f.join("api").exists());
    let web = e.root.join("repos/web");
    assert!(!has_branch(&e, &web, "feat/y"));
    assert!(!e.git(&web, &["worktree", "list"]).contains("shop"));
}

#[test]
fn rm_warns_when_cwd_is_inside_the_target() {
    let e = Env::new();
    e.project("app");
    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    let r = e.forest_in(&f.join("app"), &["rm"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stderr.contains("warning: the current directory"),
        "{}",
        r.stderr
    );

    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    let r = e.forest_in(&f, &["rm", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!r.stderr.contains("warning"), "{}", r.stderr);
    assert_eq!(r.json()["warnings"].as_array().unwrap().len(), 1);
    assert!(!f.exists());

    // Not inside: no warning.
    e.ok(&["new", "feat/x", "app"]);
    let r = e.forest(&["rm", "feat/x", "app"]);
    assert!(!r.stderr.contains("warning"), "{}", r.stderr);
}

#[test]
fn rm_and_done_need_a_worktree_or_cwd() {
    let e = Env::new();
    e.project("app");
    e.ok(&["new", "feat/x", "app"]);
    for args in [
        &["rm", "feat/x"][..],
        &["rm", "feat-x"],
        &["done", "feat/x"],
        &["done", "feat-x"],
    ] {
        let r = e.forest(args);
        assert_eq!(r.code, 1, "{args:?}");
        assert!(
            r.stderr.contains("name a worktree"),
            "{args:?}: {}",
            r.stderr
        );
        assert!(forest_dir(&e, "feat-x").exists());
    }

    // An unknown name is unknown, not a missing repo.
    for args in [&["rm", "nope"][..], &["done", "nope"]] {
        let r = e.forest(args);
        assert_eq!(r.code, 1, "{args:?}");
        assert!(r.stderr.contains("named 'nope'"), "{args:?}: {}", r.stderr);
    }

    // One worktree by name, and the whole forest from inside, still work.
    e.ok(&["rm", "feat/x", "app"]);
    let f = PathBuf::from(e.ok(&["new", "feat/y", "app"]));
    let r = e.forest_in(&f, &["rm"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!f.exists());
}

#[test]
fn done_merges_and_closes() {
    let e = Env::new();
    let up = e.upstream("svc");
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));
    e.ok(&["new", "feat/x", "svc"]);
    let x = forest_dir(&e, "feat-x").join("svc");
    e.commit(&x, "work");

    let r = e.forest_in(&x, &["done", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["forest"], "feat-x");
    let m = &j["merged"][0];
    assert_eq!(m["branch"], "feat/x");
    assert_eq!(m["into"], "main");
    assert_eq!(m["already_merged"], false);
    assert_eq!(m["branch_deleted"], false);
    assert_eq!(j["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(e.git(&repo, &["log", "-1", "--format=%s"]), "work");
    assert!(!forest_dir(&e, "feat-x").exists());
    // The branch stays; all its commits are on main now.
    assert!(has_branch(&e, &repo, "feat/x"));
}

#[test]
fn done_checks_everything_first() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.ok(&["new", "feat/x", "api", "web"]);
    let f = forest_dir(&e, "feat-x");
    e.commit(&f.join("api"), "api work");
    e.commit(&f.join("web"), "web work");
    // main of web moves ahead: the branches diverge.
    e.commit(&e.root.join("repos/web"), "home moves");

    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("diverged"), "{}", r.stderr);
    // api was mergeable, but nothing went through.
    assert_eq!(
        e.git(&e.root.join("repos/api"), &["log", "-1", "--format=%s"]),
        "first"
    );
    assert!(f.join("api").exists() && f.join("web").exists());
    assert!(has_branch(&e, &e.root.join("repos/api"), "feat/x"));

    // Without the diverged repo, the rest goes through and the forest closes.
    e.ok(&["rm", "feat/x", "web"]);
    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!f.exists());
    assert_eq!(
        e.git(&e.root.join("repos/api"), &["log", "-1", "--format=%s"]),
        "api work"
    );
}

#[test]
fn done_refuses_dirty_worktrees_and_wandering_homes() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.ok(&["clone", up.to_str().unwrap()]);
    e.ok(&["new", "feat/x", "svc"]);
    let f = forest_dir(&e, "feat-x");
    let home = e.root.join("repos/svc");

    fs::write(f.join("svc/dirty"), "x").unwrap();
    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("uncommitted changes"), "{}", r.stderr);
    fs::remove_file(f.join("svc/dirty")).unwrap();

    // origin/HEAD says the default branch is main, so a home that wandered off
    // it is refused. (Without a remote, the home's branch is by definition the
    // default one, and done merges into it.)
    e.git(&home, &["checkout", "-q", "-b", "elsewhere"]);
    e.commit(&f.join("svc"), "work");
    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 2);
    assert!(
        r.stderr
            .contains("home of svc is on 'elsewhere', not 'main'"),
        "{}",
        r.stderr
    );
    e.git(&home, &["checkout", "-q", "main"]);

    fs::write(home.join("dirty"), "x").unwrap();
    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("home of svc"), "{}", r.stderr);
    fs::remove_file(home.join("dirty")).unwrap();

    let r = e.forest_in(&f, &["done"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!f.exists());
}

#[test]
fn done_delete_branch_after_the_merge() {
    let e = Env::new();
    let up = e.upstream("svc");
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));
    e.ok(&["new", "feat/x", "svc"]);
    e.commit(&forest_dir(&e, "feat-x").join("svc"), "work");

    let r = e.forest(&["done", "feat/x", "svc", "--delete-branch", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["merged"][0]["branch_deleted"], true);
    assert!(!has_branch(&e, &repo, "feat/x"));
    assert_eq!(e.git(&repo, &["log", "-1", "--format=%s"]), "work");

    // Even a merged default branch is never deleted.
    e.git(&repo, &["checkout", "-q", "-b", "elsewhere"]);
    e.ok(&["new", "main", "svc"]);
    let r = e.forest(&["done", "main", "svc", "--delete-branch"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("never deleted"), "{}", r.stderr);
}

#[test]
fn done_closes_what_is_already_merged() {
    let e = Env::new();
    e.project("app");
    e.ok(&["new", "feat/x", "app"]);
    e.commit(&forest_dir(&e, "feat-x").join("app"), "work");
    e.git(
        &e.root.join("repos/app"),
        &["merge", "-q", "--ff-only", "feat/x"],
    );
    // main moves on: the branch is behind, but contained.
    e.commit(&e.root.join("repos/app"), "after");

    let r = e.forest(&["done", "feat/x", "app", "--delete-branch", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["merged"][0]["already_merged"], true);
    assert!(!forest_dir(&e, "feat-x").exists());
    assert!(!has_branch(&e, &e.root.join("repos/app"), "feat/x"));
}

#[test]
fn push_makes_remote_branches() {
    let e = Env::new();
    let svc = e.upstream("svc");
    let web = e.upstream("web");
    e.ok(&["clone", svc.to_str().unwrap()]);
    e.ok(&["clone", web.to_str().unwrap()]);
    e.ok(&["new", "feat/x", "svc", "web"]);
    let f = forest_dir(&e, "feat-x");
    e.commit(&f.join("svc"), "svc work");
    e.commit(&f.join("web"), "web work");

    let r = e.forest(&["push", "feat/x", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["pushes"].as_array().unwrap().len(), 2);
    assert_eq!(j["pushes"][0]["branch"], "feat/x");
    for (repo, up) in [("svc", &svc), ("web", &web)] {
        assert_eq!(
            e.git(up, &["log", "-1", "--format=%s", "feat/x"]),
            format!("{repo} work"),
        );
        assert_eq!(
            e.git(&f.join(repo), &["rev-parse", "--abbrev-ref", "@{upstream}"]),
            "origin/feat/x"
        );
    }
}

#[test]
fn push_reports_failures_per_repo() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.ok(&["clone", up.to_str().unwrap()]);
    e.project("solo"); // no remote
    e.ok(&["new", "feat/x", "svc", "solo"]);
    let f = forest_dir(&e, "feat-x");
    e.commit(&f.join("svc"), "work");
    e.commit(&f.join("solo"), "work");

    let r = e.forest(&["push", "feat/x", "--json"]);
    assert_eq!(r.code, 1);
    let j = r.json();
    let pushes = j["pushes"].as_array().unwrap().clone();
    assert_eq!(pushes.len(), 2);
    let solo = pushes.iter().find(|p| p["project"] == "solo").unwrap();
    assert!(solo["error"].is_string());
    let svc = pushes.iter().find(|p| p["project"] == "svc").unwrap();
    assert!(svc["error"].is_null());
    // The repo that could push, did.
    assert_eq!(e.git(&up, &["log", "-1", "--format=%s", "feat/x"]), "work");
}

#[test]
fn mv_renames_a_forest() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let old = PathBuf::from(e.ok(&["new", "feat/a", "api", "web"]));
    let shop = forest_dir(&e, "shop");

    let r = e.forest(&["mv", "feat/a", "shop", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["renamed_from"], "feat-a");
    assert!(!old.exists());
    // git follows: the worktrees work, and are found where they are now.
    assert_eq!(
        e.git(&shop.join("web"), &["branch", "--show-current"]),
        "feat/a"
    );
    assert!(
        e.git(&e.root.join("repos/api"), &["worktree", "list"])
            .contains("forests/shop/api")
    );
    assert_eq!(e.ok(&["path", "feat/a"]), shop.to_str().unwrap());
    e.ok(&["rm", "shop", "web"]);

    assert_eq!(e.forest(&["mv", "shop", "api"]).code, 1);
    e.ok(&["new", "feat/b", "web"]);
    assert_eq!(e.forest(&["mv", "shop", "feat-b"]).code, 1);
    assert_eq!(e.forest(&["mv", "api", "x"]).code, 1);
    // Back to its branch's name.
    e.ok(&["mv", "shop", "feat-a"]);
    assert!(old.join("api").is_dir());
}

#[test]
fn clean_removes_only_finished_forests() {
    let e = Env::new();
    let up = e.upstream("svc");
    let repo = PathBuf::from(e.ok(&["clone", up.to_str().unwrap()]));
    let wt = |b: &str| PathBuf::from(e.ok(&["new", b, "svc"])).join("svc");

    // Merged into main: done.
    let m = wt("feat/merged");
    e.commit(&m, "merged work");
    e.git(&repo, &["merge", "-q", "--ff-only", "feat/merged"]);
    // Just made, nothing done yet: kept, though main has all of it.
    wt("feat/fresh");
    // Merged, but with uncommitted changes: kept.
    let d = wt("feat/dirty");
    e.commit(&d, "dirty work");
    e.git(&repo, &["merge", "-q", "--no-edit", "feat/dirty"]);
    fs::write(d.join("notes"), "x").unwrap();
    // Upstream gone with a commit never pushed (a squash merge, say): done, but its
    // branch must stay.
    e.git(&up, &["branch", "feat/gone"]);
    let g = wt("feat/gone");
    e.commit(&g, "squashed");
    e.git(&up, &["branch", "-D", "feat/gone"]);
    e.git(&repo, &["fetch", "-q", "--prune"]);
    // Unmerged work: kept.
    let u = wt("feat/open");
    e.commit(&u, "open work");

    let r = e.forest(&["clean", "--dry-run", "--delete-branch", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    let names = |k: &str| -> Vec<String> {
        j[k].as_array()
            .unwrap()
            .iter()
            .map(|f| f["forest"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names("removed"), ["feat-gone", "feat-merged"]);
    assert_eq!(names("kept"), ["feat-dirty", "feat-fresh", "feat-open"]);
    assert!(forest_dir(&e, "feat-merged").exists());

    let out = e.ok(&["clean", "--delete-branch"]);
    assert!(out.contains("deleted branch feat/merged of svc"), "{out}");
    assert!(
        out.contains("kept branch feat/gone of svc: 1 commits"),
        "{out}"
    );
    assert!(!forest_dir(&e, "feat-merged").exists());
    assert!(!forest_dir(&e, "feat-gone").exists());
    assert!(!has_branch(&e, &repo, "feat/merged"));
    assert!(has_branch(&e, &repo, "feat/gone"));
    for f in ["feat-dirty", "feat-fresh", "feat-open"] {
        assert!(forest_dir(&e, f).exists(), "{f}");
    }
}

#[test]
fn prune_reports_orphans() {
    let e = Env::new();
    e.project("app");
    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    fs::rename(f.join("app"), f.join("moved")).unwrap();
    let empty = forest_dir(&e, "leftover");
    fs::create_dir_all(&empty).unwrap();

    let j = e.forest(&["prune", "--json"]).json();
    assert_eq!(j["pruned"][0]["project"], "app");
    let orphans: Vec<_> = j["orphans"].as_array().unwrap().to_vec();
    assert!(orphans.contains(&Value::from(f.join("moved").to_str().unwrap())));
    assert!(orphans.contains(&Value::from(empty.to_str().unwrap())));
}

#[test]
fn ls_shows_repos_forests_and_the_rest() {
    let e = Env::new();
    let app = e.project("app");
    e.project("lib");
    e.ok(&["new", "feat/x", "app", "lib", "--as", "both"]);
    e.ok(&["new", "feat/y", "lib"]);
    let outside = e.base.join("outside");
    e.git(
        &app,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "tmp",
            outside.to_str().unwrap(),
        ],
    );

    let out = e.ok(&["ls"]);
    assert!(out.starts_with("repos\n  app  main"), "{out}");
    assert!(out.contains("forests\n  both    feat/x  app"), "{out}");
    assert!(out.contains("\n  feat-y          lib"), "{out}");
    assert!(out.contains("outside the layout\n  app  tmp"), "{out}");

    // One repo: its home, the forests it is in, and its other worktrees.
    let j = e.forest(&["ls", "app", "--json"]).json();
    assert_eq!(j["repos"].as_array().unwrap().len(), 1);
    assert_eq!(j["forests"][0]["name"], "both");
    assert_eq!(
        j["forests"][0]["projects"],
        serde_json::json!(["app", "lib"])
    );
    assert_eq!(j["worktrees"].as_array().unwrap().len(), 4);
    // One forest, by branch.
    let j = e.forest(&["ls", "feat/y", "--json"]).json();
    assert_eq!(j["repos"].as_array().unwrap().len(), 0);
    assert_eq!(j["forests"][0]["name"], "feat-y");

    // Usage errors are 1, never 2 (which means "refused").
    assert_eq!(e.forest(&["bogus"]).code, 1);
    assert_eq!(e.forest(&["--help"]).code, 0);
}

#[test]
fn ls_survives_a_broken_worktree() {
    let e = Env::new();
    e.project("app");
    let x = PathBuf::from(e.ok(&["new", "feat/x", "app"])).join("app");
    fs::write(x.join(".git"), "gitdir: /nonexistent\n").unwrap();
    let r = e.forest(&["ls", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let rows = r.json()["worktrees"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 2);
    let broken = rows.iter().find(|w| w["branch"] == "feat/x").unwrap();
    assert!(broken["error"].is_string());
    let main = rows.iter().find(|w| w["branch"] == "main").unwrap();
    assert!(main["error"].is_null());
    assert!(e.ok(&["ls"]).contains("error: "));
}

#[test]
fn new_recreates_a_deleted_worktree() {
    let e = Env::new();
    e.project("app");
    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    fs::remove_dir_all(f.join("app")).unwrap();
    let r = e.forest(&["new", "feat/x", "app", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(f.join("app/.git").exists());
}

fn identity(kind: &str, forest: Value, project: Value, branch: Value, path: &Path) -> Value {
    serde_json::json!({
        "v": 1,
        "kind": kind,
        "forest": forest,
        "project": project,
        "branch": branch,
        "path": path.to_str().unwrap(),
    })
}

/// The identity fields of a JSON result, in order, without the command's own fields.
fn identity_of(j: &Value) -> Value {
    let keys = ["v", "kind", "forest", "project", "branch", "path"];
    let obj = j.as_object().unwrap();
    let got: Vec<_> = obj.keys().take(keys.len()).map(String::as_str).collect();
    assert_eq!(got, keys, "{j}");
    Value::Object(
        obj.iter()
            .filter(|(k, _)| keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

#[test]
fn json_identity_of_a_forest() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    let f = forest_dir(&e, "shop");
    let want = identity("forest", "shop".into(), Value::Null, "feat/y".into(), &f);

    let r = e.forest(&["new", "feat/y", "api", "web", "--as", "shop", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["worktrees"].as_array().unwrap().len(), 2);

    for r in [
        e.forest(&["path", "shop", "--json"]),
        e.forest(&["path", "feat/y", "--json"]),
        e.forest_in(&f, &["path", "--json"]),
        e.forest_in(&f.join("web"), &["path", "--json"]),
    ] {
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(r.json(), want, "{}", r.stdout);
    }

    let r = e.forest_in(&f, &["rm", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["removed"].as_array().unwrap().len(), 2);
    assert!(!f.exists());
}

#[test]
fn json_identity_of_worktrees() {
    let e = Env::new();
    let api = e.project("api");
    let f = PathBuf::from(e.ok(&["new", "feat/y", "api"]));
    let member = identity(
        "worktree",
        "feat-y".into(),
        "api".into(),
        "feat/y".into(),
        &f.join("api"),
    );
    for args in [&["path", "feat-y", "api"][..], &["path", "feat/y", "api"]] {
        let mut args = args.to_vec();
        args.push("--json");
        assert_eq!(e.forest(&args).json(), member, "{args:?}");
    }
    let home = identity("worktree", Value::Null, "api".into(), "main".into(), &api);
    for r in [
        e.forest(&["path", "api", "--json"]),
        e.forest(&["path", "main", "api", "--json"]),
        e.forest_in(&api, &["path", "--json"]),
    ] {
        assert_eq!(r.json(), home, "{}", r.stdout);
    }
}

#[test]
fn json_identity_of_a_forest_without_a_branch() {
    let e = Env::new();
    let api = e.project("api");
    let f = forest_dir(&e, "shop");
    fs::create_dir_all(&f).unwrap();
    let want = identity("forest", "shop".into(), Value::Null, Value::Null, &f);
    assert_eq!(e.forest(&["path", "shop", "--json"]).json(), want);

    // A detached worktree doesn't give it a branch either.
    let member = f.join("api");
    e.git(
        &api,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            member.to_str().unwrap(),
            "main",
        ],
    );
    // `path` steps into the only worktree, which has no branch either.
    let wt = identity(
        "worktree",
        "shop".into(),
        "api".into(),
        Value::Null,
        &member,
    );
    assert_eq!(e.forest_in(&member, &["path", "--json"]).json(), wt);
    let ls = e.forest(&["ls", "shop", "--json"]).json();
    assert_eq!(ls["forests"][0]["branch"], Value::Null);
    assert_eq!(ls["forests"][0]["projects"][0], "api");
}

#[test]
fn json_errors() {
    let e = Env::new();
    e.project("app");
    for args in [
        &["path", "nope", "--json"][..],
        &["rm", "nope", "app", "--json"],
        &["new", "x", "nope", "--json"],
    ] {
        let r = e.forest(args);
        assert_eq!(r.code, 1);
        let j = r.json();
        let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["v", "error"], "{j}");
        assert_eq!(j["error"]["code"], "error");
        assert!(j["error"]["message"].is_string());
    }
    let f = PathBuf::from(e.ok(&["new", "feat/x", "app"]));
    fs::write(f.join("app/dirty"), "x").unwrap();
    let r = e.forest(&["rm", "feat/x", "app", "--json"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["code"], "refused");
}

#[test]
fn migrate_a_clone_moves_it_and_its_worktrees() {
    let e = Env::new();
    let src = e.upstream("svc");
    let linked = e.base.join("elsewhere");
    e.git(
        &src,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            linked.to_str().unwrap(),
        ],
    );
    fs::write(src.join("untracked"), "u").unwrap();
    // A detached HEAD moves along with the repo.
    e.git(&src, &["checkout", "-q", "--detach"]);

    let r = e.forest(&["migrate", src.to_str().unwrap(), "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let dest = e.root.join("repos/svc");
    assert_eq!(r.json()["path"], dest.to_str().unwrap());
    assert!(!src.exists() && !linked.exists());
    assert_eq!(fs::read_to_string(dest.join("untracked")).unwrap(), "u");
    let x = forest_dir(&e, "feat-x").join("svc");
    assert_eq!(e.git(&x, &["branch", "--show-current"]), "feat/x");
    assert_eq!(e.ok(&["path", "feat/x", "svc"]), x.to_str().unwrap());

    // Running it again changes nothing.
    let out = e.ok(&["migrate", dest.to_str().unwrap()]);
    assert!(out.contains("already in the layout"), "{out}");
}

#[test]
fn migrate_the_older_layout() {
    let e = Env::new();
    let up = e.upstream("svc");
    let old = e.root.join("svc");
    let bare = old.join(".bare");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    e.git(&bare, &["worktree", "add", "-q", "../main", "main"]);
    e.git(
        &bare,
        &["worktree", "add", "-q", "-b", "feat/x", "../feat-x"],
    );
    // A worktree some tool made inside the repo stays there.
    e.git(
        &bare,
        &["worktree", "add", "-q", "-b", "agent", ".tool/agent"],
    );
    let main = old.join("main");
    fs::write(main.join("staged"), "s").unwrap();
    e.git(&main, &["add", "staged"]);
    fs::write(main.join("untracked"), "u").unwrap();
    fs::write(old.join("feat-x/wip"), "w").unwrap();

    let r = e.forest(&["migrate", old.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!old.exists());
    let dest = e.root.join("repos/svc");
    assert!(dest.join(".git").is_dir());
    assert_eq!(e.git(&dest, &["branch", "--show-current"]), "main");
    assert_eq!(
        e.git(&dest, &["status", "--porcelain"]),
        "A  staged\n?? untracked"
    );
    let x = forest_dir(&e, "feat-x").join("svc");
    assert_eq!(fs::read_to_string(x.join("wip")).unwrap(), "w");
    assert_eq!(e.git(&x, &["branch", "--show-current"]), "feat/x");
    let agent = dest.join(".git/.tool/agent");
    assert_eq!(e.git(&agent, &["branch", "--show-current"]), "agent");
    // A bare clone's branches become remote-tracking ones on the next fetch.
    assert_eq!(
        e.git(&dest, &["config", "remote.origin.fetch"]),
        "+refs/heads/*:refs/remotes/origin/*"
    );
    e.ok(&["new", "feat/y", "svc"]);
}

#[test]
fn migrate_a_bare_repo() {
    let e = Env::new();
    let up = e.upstream("lib");
    // Without worktrees: the default branch is checked out.
    let src = e.base.join("lib.git");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    );
    e.ok(&["migrate", src.to_str().unwrap()]);
    let dest = e.root.join("repos/lib");
    assert!(!src.exists());
    assert_eq!(e.git(&dest, &["log", "-1", "--format=%s"]), "first");
    assert_eq!(e.git(&dest, &["status", "--porcelain"]), "");

    // With a worktree inside on the default branch: that becomes the checkout.
    let src = e.base.join("other.git");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    );
    e.git(&src, &["worktree", "add", "-q", "wt", "main"]);
    fs::write(src.join("wt/dirty"), "d").unwrap();
    e.ok(&["migrate", src.to_str().unwrap()]);
    let dest = e.root.join("repos/other");
    assert_eq!(fs::read_to_string(dest.join("dirty")).unwrap(), "d");
    assert!(!src.exists());
}

#[test]
fn migrate_refusals() {
    let e = Env::new();
    // An operation in progress in the worktree that would become the checkout.
    let up = e.upstream("svc");
    let old = e.base.join("svc");
    let bare = old.join(".bare");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    e.git(&bare, &["worktree", "add", "-q", "../main", "main"]);
    fs::write(bare.join("worktrees/main/MERGE_HEAD"), "x").unwrap();
    let r = e.forest(&["migrate", old.to_str().unwrap()]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(bare.is_dir() && !e.root.join("repos/svc").exists());

    // A linked worktree is not a repo of its own.
    let r = e.forest(&["migrate", old.join("main").to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("linked worktree"), "{}", r.stderr);

    // The name is taken.
    e.project("x");
    let src = e.upstream("x");
    let r = e.forest(&["migrate", src.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("already exists"), "{}", r.stderr);
}
