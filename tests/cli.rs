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

    fn forest_toml(&self, name: &str, projects: &str) {
        let dir = self.root.join("_forests").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".forest.toml"), format!("projects = {projects}\n")).unwrap();
    }

    /// `init` + first commit, so branches can be made.
    fn project(&self, name: &str) -> PathBuf {
        let wt = PathBuf::from(self.ok(&["init", name]));
        self.commit(&wt, "first");
        wt
    }
}

#[test]
fn init_creates_unborn_default_worktree() {
    let e = Env::new();
    let wt = e.ok(&["init", "app", "--branch", "trunk"]);
    assert_eq!(PathBuf::from(&wt), e.root.join("app/trunk"));
    assert!(e.root.join("app/.bare").is_dir());

    let ls = e.forest(&["ls", "--json"]).json();
    assert_eq!(ls["v"], 1);
    assert_eq!(ls["worktrees"][0]["branch"], "trunk");
    assert_eq!(ls["worktrees"][0]["head"], Value::Null);

    let r = e.forest(&["new", "app:feat/x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("no commits yet"), "{}", r.stderr);
}

#[test]
fn init_rejects_bad_names() {
    let e = Env::new();
    assert_eq!(e.forest(&["init", "_x"]).code, 1);
    assert_eq!(e.forest(&["init", ".x"]).code, 1);
    e.forest_toml("shop", "[]");
    assert_eq!(e.forest(&["init", "shop"]).code, 1);
    e.ok(&["init", "app"]);
    assert_eq!(e.forest(&["init", "app"]).code, 1);
}

#[test]
fn clone_sets_up_normal_remote_layout() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.git(&up, &["branch", "-m", "main", "develop"]);
    e.git(&up, &["branch", "feat/remote"]);
    let wt = e.ok(&["clone", up.to_str().unwrap()]);
    assert_eq!(PathBuf::from(&wt), e.root.join("svc/develop"));

    let bare = e.root.join("svc/.bare");
    assert_eq!(
        e.git(&bare, &["symbolic-ref", "HEAD"]),
        "refs/heads/develop"
    );
    assert_eq!(
        e.git(&bare, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/develop"
    );
    // Only the default branch is local; the rest stays remote-tracking.
    assert_eq!(
        e.git(&bare, &["branch", "--format=%(refname:short)"]),
        "develop"
    );
    assert_eq!(
        e.git(
            &PathBuf::from(&wt),
            &["rev-parse", "--abbrev-ref", "@{upstream}"]
        ),
        "origin/develop"
    );
    // Relative worktree paths.
    let gitfile = fs::read_to_string(PathBuf::from(&wt).join(".git")).unwrap();
    assert!(gitfile.contains("../.bare"), "{gitfile}");
}

#[test]
fn new_resolves_local_remote_and_new_branches() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.git(&up, &["branch", "feat/remote"]);
    e.ok(&["clone", up.to_str().unwrap()]);
    let bare = e.root.join("svc/.bare");

    // Remote branch: tracking set up.
    let wt = PathBuf::from(e.ok(&["new", "svc:feat/remote"]));
    assert_eq!(wt, e.root.join("svc/feat-remote"));
    assert_eq!(
        e.git(&wt, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/feat/remote"
    );

    // New branch from the default branch, without an upstream.
    let wt = PathBuf::from(e.ok(&["new", "svc:feat/new"]));
    assert_eq!(wt, e.root.join("svc/feat-new"));
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
    let wt2 = PathBuf::from(e.ok(&["new", "svc:feat/stacked", "--from", "feat/new"]));
    assert_eq!(e.git(&wt2, &["log", "-1", "--format=%s"]), "on new");

    // Local branch without a worktree.
    e.git(&bare, &["branch", "local-only", "feat/new"]);
    let wt3 = PathBuf::from(e.ok(&["new", "svc:local-only"]));
    assert_eq!(e.git(&wt3, &["log", "-1", "--format=%s"]), "on new");

    // Unknown --from on a project is an error.
    assert_eq!(e.forest(&["new", "svc:x", "--from", "nope"]).code, 1);
}

#[test]
fn new_is_idempotent_and_addresses_by_dir_name() {
    let e = Env::new();
    e.project("app");
    let a = e.ok(&["new", "app:feat/x"]);
    assert_eq!(e.ok(&["new", "app:feat/x"]), a);
    assert_eq!(e.ok(&["new", "app:feat-x"]), a);
    assert_eq!(e.ok(&["path", "app:feat-x"]), a);
    assert_eq!(e.ok(&["path", "app:feat/x"]), a);
}

#[test]
fn new_rejects_dir_collision_and_double_checkout() {
    let e = Env::new();
    e.project("app");
    e.ok(&["new", "app:feat-x"]);
    let r = e.forest(&["new", "app:feat/x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("collides"), "{}", r.stderr);

    e.forest_toml("shop", r#"["app"]"#);
    let r = e.forest(&["new", "shop:main"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("already checked out at"), "{}", r.stderr);
}

#[test]
fn cwd_context() {
    let e = Env::new();
    let main = e.project("app");
    let sub = main.join("deep/dir");
    fs::create_dir_all(&sub).unwrap();

    let r = e.forest_in(&sub, &["new", "feat/x"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), e.root.join("app/feat-x").to_str().unwrap());

    let r = e.forest_in(&sub, &["path"]);
    assert_eq!(r.stdout.trim(), main.to_str().unwrap());

    let r = e.forest_in(&e.root.join("app"), &["path"]);
    assert_eq!(r.code, 1);

    let r = e.forest_in(&e.base, &["path", "feat/x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("<name>:<branch>"), "{}", r.stderr);
}

#[test]
fn workforest_fills_gaps_and_reports_partial_failure() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.ok(&["init", "empty"]); // unborn: can't branch
    e.forest_toml("shop", r#"["api", "web", "empty"]"#);

    let r = e.forest(&["new", "shop:feat/y", "--json"]);
    assert_eq!(r.code, 1);
    let j = r.json();
    let wf = e.root.join("_forests/shop/feat-y");
    assert_eq!(j["path"], wf.to_str().unwrap());
    let statuses: Vec<_> = j["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["status"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(statuses, ["created", "created", "failed"]);
    assert!(wf.join("api/.git").exists());

    // Fix the failing project and re-run, addressed by dir name: fills the gap
    // with the workforest's real branch.
    e.commit(&e.root.join("empty/main"), "first");
    let r = e.forest(&["new", "shop:feat-y", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["branch"], "feat/y");
    assert_eq!(j["worktrees"][0]["status"], "exists");
    assert_eq!(j["worktrees"][2]["status"], "created");
    assert_eq!(
        e.git(&wf.join("empty"), &["branch", "--show-current"]),
        "feat/y"
    );

    // Inside a workforest, path means the whole workforest.
    let r = e.forest_in(&wf.join("web"), &["path"]);
    assert_eq!(r.stdout.trim(), wf.to_str().unwrap());

    // --from is used where it exists, else the default branch.
    e.commit(&wf.join("api"), "api work");
    e.ok(&["new", "shop:feat/z", "--from", "feat/y"]);
    let z = e.root.join("_forests/shop/feat-z");
    assert_eq!(
        e.git(&z.join("api"), &["log", "-1", "--format=%s"]),
        "api work"
    );
    assert_eq!(
        e.git(&z.join("web"), &["log", "-1", "--format=%s"]),
        "first"
    );

    let ls = e.forest(&["ls", "api", "--json"]).json();
    let forests: Vec<_> = ls["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["forest"].clone())
        .collect();
    assert!(forests.contains(&Value::from("shop")));
    assert!(forests.contains(&Value::Null));
}

#[test]
fn name_that_is_both_project_and_forest_is_an_error() {
    let e = Env::new();
    e.project("app");
    e.forest_toml("app", r#"["app"]"#);
    let r = e.forest(&["new", "app:x"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("both"), "{}", r.stderr);
}

#[test]
fn rm_refuses_dirty_and_default_worktrees() {
    let e = Env::new();
    let main = e.project("app");
    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
    fs::write(x.join("new.txt"), "x").unwrap();

    let r = e.forest(&["rm", "app:feat/x", "--json"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["code"], "refused");
    assert!(x.exists());

    // Ignored files don't count.
    fs::remove_file(x.join("new.txt")).unwrap();
    fs::write(x.join(".gitignore"), "*.log\n").unwrap();
    e.git(&x, &["add", ".gitignore"]);
    e.commit(&x, "ignore");
    fs::write(x.join("a.log"), "x").unwrap();
    e.ok(&["rm", "app:feat/x"]);
    assert!(!x.exists());
    // The branch is kept.
    e.git(
        &e.root.join("app/.bare"),
        &["rev-parse", "--verify", "feat/x"],
    );

    assert_eq!(e.forest(&["rm", "app:main"]).code, 2);
    e.ok(&["rm", "app:main", "--force"]);
    assert!(!main.exists());
}

#[test]
fn rm_delete_branch_safety() {
    let e = Env::new();
    let up = e.upstream("svc");
    e.ok(&["clone", up.to_str().unwrap()]);
    let bare = e.root.join("svc/.bare");
    let has_branch = |b: &str| !e.git(&bare, &["branch", "--list", b]).is_empty();

    // Unpushed commits: refused.
    let a = PathBuf::from(e.ok(&["new", "svc:feat/a"]));
    e.commit(&a, "unpushed");
    assert_eq!(e.forest(&["rm", "svc:feat/a", "--delete-branch"]).code, 2);
    assert!(a.exists());

    // Nothing beyond main: safe.
    e.ok(&["new", "svc:feat/b"]);
    e.ok(&["rm", "svc:feat/b", "--delete-branch"]);
    assert!(!has_branch("feat/b"));

    // Upstream gone after a squash merge: safe.
    e.git(&up, &["branch", "feat/c"]);
    let c = PathBuf::from(e.ok(&["new", "svc:feat/c"]));
    e.commit(&c, "squashed upstream");
    e.git(&up, &["branch", "-D", "feat/c"]);
    e.ok(&["new", "svc:feat/d"]); // fetches with --prune
    let ls = e.forest(&["ls", "svc", "--json"]).json();
    let row = ls["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["branch"] == "feat/c")
        .unwrap()
        .clone();
    assert_eq!(row["upstream_gone"], true);
    e.ok(&["rm", "svc:feat/c", "--delete-branch"]);
    assert!(!has_branch("feat/c"));

    // --force overrides.
    e.ok(&["rm", "svc:feat/a", "--delete-branch", "--force"]);
    assert!(!has_branch("feat/a"));
}

#[test]
fn workforest_rm_is_all_or_nothing() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.forest_toml("shop", r#"["api", "web"]"#);
    let wf = PathBuf::from(e.ok(&["new", "shop:feat/y"]));
    fs::write(wf.join("web/dirty"), "x").unwrap();

    let r = e.forest_in(&wf.join("api"), &["rm"]);
    assert_eq!(r.code, 2);
    assert!(wf.join("api").exists() && wf.join("web").exists());

    fs::remove_file(wf.join("web/dirty")).unwrap();
    let r = e.forest_in(&wf.join("api"), &["rm"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!wf.exists());
}

#[test]
fn prune_reports_orphans() {
    let e = Env::new();
    e.project("app");
    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
    fs::rename(&x, e.root.join("app/moved")).unwrap();
    fs::create_dir_all(e.root.join("app/junk")).unwrap();

    let j = e.forest(&["prune", "--json"]).json();
    assert_eq!(j["pruned"][0]["project"], "app");
    let orphans: Vec<_> = j["orphans"].as_array().unwrap().to_vec();
    assert!(orphans.contains(&Value::from(e.root.join("app/junk").to_str().unwrap())));
    assert!(orphans.contains(&Value::from(e.root.join("app/moved").to_str().unwrap())));
}

#[test]
fn ls_human_and_usage_errors() {
    let e = Env::new();
    e.project("app");
    e.ok(&["new", "app:feat/x"]);
    let out = e.ok(&["ls"]);
    assert!(out.starts_with("app  (default: main, no origin)"), "{out}");
    assert!(out.contains("feat-x  feat/x"), "{out}");

    // Usage errors are 1, never 2 (which means "refused").
    assert_eq!(e.forest(&["bogus"]).code, 1);
    assert_eq!(e.forest(&["--help"]).code, 0);
}

#[test]
fn define_creates_and_edits_forests() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.project("docs");
    let toml = e.root.join("_forests/shop/.forest.toml");

    // Creating needs projects that exist, and a name no project uses.
    assert_eq!(e.forest(&["define", "shop"]).code, 1);
    assert_eq!(e.forest(&["define", "shop", "api", "nope"]).code, 1);
    assert_eq!(e.forest(&["define", "api", "web"]).code, 1);
    assert_eq!(e.forest(&["define", "shop", "--add", "api"]).code, 1);
    assert!(!toml.exists());

    assert_eq!(
        e.ok(&["define", "shop", "api", "web", "api"]),
        "shop  [api, web]"
    );
    assert_eq!(
        fs::read_to_string(&toml).unwrap(),
        "projects = [\"api\", \"web\"]\n"
    );
    e.ok(&["new", "shop:feat/y"]);

    // An existing forest only changes through --add/--remove.
    assert_eq!(e.forest(&["define", "shop", "docs"]).code, 1);
    assert_eq!(e.forest(&["define", "shop", "--remove", "docs"]).code, 1);
    let j = e
        .forest(&[
            "define", "shop", "--add", "docs", "--remove", "web", "--json",
        ])
        .json();
    assert_eq!(j["projects"], serde_json::json!(["api", "docs"]));
    assert_eq!(j["created"], false);
    assert_eq!(
        e.forest(&["define", "shop", "--remove", "api", "docs"])
            .code,
        1
    );

    // Re-running new fills in the added project.
    let wf = PathBuf::from(e.ok(&["new", "shop:feat/y"]));
    assert!(wf.join("docs/.git").exists());
}

#[test]
fn new_recreates_a_deleted_worktree() {
    let e = Env::new();
    e.project("app");
    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
    fs::remove_dir_all(&x).unwrap();
    let r = e.forest(&["new", "app:feat/x", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["worktrees"][0]["status"], "created");
    assert!(x.join(".git").exists());
}

#[test]
fn workforest_rm_finds_missing_members_and_keeps_strays() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.forest_toml("shop", r#"["api", "web"]"#);
    let wf = PathBuf::from(e.ok(&["new", "shop:feat/y"]));
    fs::remove_dir_all(wf.join("web")).unwrap();
    fs::create_dir_all(wf.join("notes")).unwrap();
    fs::write(wf.join("notes/todo"), "x").unwrap();

    // The stray dir refuses without --force.
    assert_eq!(e.forest(&["rm", "shop:feat/y"]).code, 2);

    // With --force, both worktrees go, but the stray dir is kept.
    let r = e.forest(&["rm", "shop:feat/y", "--force", "--delete-branch", "--json"]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    let j = r.json();
    assert_eq!(j["removed"].as_array().unwrap().len(), 2);
    assert_eq!(j["errors"].as_array().unwrap().len(), 1);
    assert!(wf.join("notes/todo").exists());
    assert!(!wf.join("api").exists());
    let web_bare = e.root.join("web/.bare");
    assert!(e.git(&web_bare, &["branch", "--list", "feat/y"]).is_empty());
    assert!(!e.git(&web_bare, &["worktree", "list"]).contains("feat-y"));

    // Nothing still holds the branch, so the workforest can be made again.
    e.ok(&["new", "shop:feat/y"]);
}

#[test]
fn target_prefers_branch_over_dir_name() {
    let e = Env::new();
    e.project("app");
    let bare = e.root.join("app/.bare");
    let b = e.root.join("app/b");
    let a = e.root.join("app/a");
    e.git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "c",
            b.to_str().unwrap(),
            "main",
        ],
    );
    e.git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b",
            a.to_str().unwrap(),
            "main",
        ],
    );
    assert_eq!(e.ok(&["path", "app:b"]), a.to_str().unwrap());
    // A dir name still works when no branch has that name.
    e.git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/d",
            e.root.join("app/d").to_str().unwrap(),
            "main",
        ],
    );
    assert_eq!(
        e.ok(&["path", "app:d"]),
        e.root.join("app/d").to_str().unwrap()
    );
}

#[test]
fn ls_survives_a_broken_worktree() {
    let e = Env::new();
    e.project("app");
    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
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
fn prune_reports_workforest_without_worktrees() {
    let e = Env::new();
    e.project("api");
    e.forest_toml("shop", r#"["api"]"#);
    let leftover = e.root.join("_forests/shop/feat-y");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join(".envrc"), "x").unwrap();
    let j = e.forest(&["prune", "--json"]).json();
    assert_eq!(j["orphans"][0], leftover.to_str().unwrap());
}

fn identity(kind: &str, name: &str, dir: &str, branch: Value, path: &Path) -> Value {
    serde_json::json!({
        "v": 1,
        "kind": kind,
        "name": name,
        "dir": dir,
        "branch": branch,
        "path": path.to_str().unwrap(),
    })
}

/// The identity fields of a JSON result, in order, without the command's own fields.
fn identity_of(j: &Value) -> Value {
    let keys = ["v", "kind", "name", "dir", "branch", "path"];
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
fn json_identity_of_a_worktree() {
    let e = Env::new();
    e.project("app");
    let x = e.root.join("app/feat-x");
    let want = identity("worktree", "app", "feat-x", "feat/x".into(), &x);

    let r = e.forest(&["new", "app:feat/x", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["worktrees"][0]["status"], "created");
    // Re-run by dir name: same identity, real branch.
    assert_eq!(
        identity_of(&e.forest(&["new", "app:feat-x", "--json"]).json()),
        want
    );

    let deep = x.join("deep");
    fs::create_dir_all(&deep).unwrap();
    let runs = [
        e.forest(&["path", "app:feat/x", "--json"]),
        e.forest(&["path", "app:feat-x", "--json"]),
        e.forest_in(&e.root.join("app/main"), &["path", "feat/x", "--json"]),
        e.forest_in(&e.root.join("app"), &["path", "feat-x", "--json"]),
        e.forest_in(&x, &["path", "--json"]),
        e.forest_in(&deep, &["path", "--json"]),
    ];
    for r in &runs {
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(r.json(), want, "{}", r.stdout);
    }
    fs::remove_dir(&deep).unwrap();

    let r = e.forest(&["rm", "app:feat/x", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["removed"][0]["path"], x.to_str().unwrap());
    assert!(j.get("warnings").is_none(), "{j}");
}

#[test]
fn json_identity_of_a_workforest() {
    let e = Env::new();
    e.project("api");
    e.project("web");
    e.forest_toml("shop", r#"["api", "web"]"#);
    let wf = e.root.join("_forests/shop/feat-y");
    let want = identity("workforest", "shop", "feat-y", "feat/y".into(), &wf);

    let r = e.forest(&["new", "shop:feat/y", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["worktrees"].as_array().unwrap().len(), 2);

    let runs = [
        e.forest(&["path", "shop:feat/y", "--json"]),
        e.forest(&["path", "shop:feat-y", "--json"]),
        e.forest_in(&e.root.join("_forests/shop"), &["path", "feat/y", "--json"]),
        e.forest_in(&wf.join("api"), &["path", "feat-y", "--json"]),
        e.forest_in(&wf, &["path", "--json"]),
        e.forest_in(&wf.join("web"), &["path", "--json"]),
    ];
    for r in &runs {
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(r.json(), want, "{}", r.stdout);
    }

    let r = e.forest(&["rm", "shop:feat/y", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let j = r.json();
    assert_eq!(identity_of(&j), want);
    assert_eq!(j["removed"].as_array().unwrap().len(), 2);
}

#[test]
fn json_identity_of_a_workforest_without_a_branch() {
    let e = Env::new();
    e.project("api");
    e.forest_toml("shop", r#"["api"]"#);
    let wf = e.root.join("_forests/shop/feat-big");
    fs::create_dir_all(&wf).unwrap();
    let want = identity("workforest", "shop", "feat-big", Value::Null, &wf);
    assert_eq!(e.forest(&["path", "shop:feat-big", "--json"]).json(), want);
    assert_eq!(e.forest_in(&wf, &["path", "--json"]).json(), want);

    // A detached member doesn't give it a branch either.
    let bare = e.root.join("api/.bare");
    let member = wf.join("api");
    e.git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            member.to_str().unwrap(),
            "main",
        ],
    );
    assert_eq!(e.forest_in(&member, &["path", "--json"]).json(), want);
    let ls = e.forest(&["ls", "shop", "--json"]).json();
    assert_eq!(ls["forests"][0]["workforests"][0]["branch"], Value::Null);
}

#[test]
fn json_errors_are_unchanged() {
    let e = Env::new();
    e.project("app");
    for args in [
        &["path", "app:nope", "--json"][..],
        &["rm", "app:nope", "--json"],
        &["new", "nope:x", "--json"],
    ] {
        let r = e.forest(args);
        assert_eq!(r.code, 1);
        let j = r.json();
        let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["v", "error"], "{j}");
        assert_eq!(j["error"]["code"], "error");
        assert!(j["error"]["message"].is_string());
    }
    let r = e.forest(&["rm", "app:main", "--json"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["code"], "refused");
}

#[test]
fn rm_warns_when_cwd_is_inside_the_target() {
    let e = Env::new();
    e.project("app");
    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
    let r = e.forest_in(&x, &["rm"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stderr.contains("warning: the current directory"),
        "{}",
        r.stderr
    );

    let x = PathBuf::from(e.ok(&["new", "app:feat/x"]));
    let r = e.forest_in(&x, &["rm", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!r.stderr.contains("warning"), "{}", r.stderr);
    let warnings = r.json()["warnings"].as_array().unwrap().clone();
    assert_eq!(warnings.len(), 1);
    assert!(!x.exists());

    // Not inside: no warning.
    e.ok(&["new", "app:feat/x"]);
    let r = e.forest(&["rm", "app:feat/x"]);
    assert!(!r.stderr.contains("warning"), "{}", r.stderr);
}

#[test]
fn migrate_normal_clone_keeps_the_working_tree() {
    let e = Env::new();
    let up = e.upstream("svc");
    let dir = e.root.join("svc");
    e.git(
        &e.base,
        &["clone", "-q", up.to_str().unwrap(), dir.to_str().unwrap()],
    );
    e.git(&dir, &["checkout", "-q", "-b", "master"]);
    let linked = e.base.join("elsewhere");
    e.git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            linked.to_str().unwrap(),
        ],
    );
    fs::write(dir.join(".gitignore"), "build/\n").unwrap();
    fs::create_dir(dir.join("build")).unwrap();
    fs::write(dir.join("build/out"), "x").unwrap();
    fs::write(dir.join("staged"), "s").unwrap();
    e.git(&dir, &["add", "staged"]);
    // A top-level dir named like the branch dir must not get in the way.
    fs::create_dir(dir.join("master")).unwrap();
    fs::write(dir.join("master/file"), "m").unwrap();

    let r = e.forest(&["migrate", dir.to_str().unwrap(), "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["path"], dir.to_str().unwrap());

    let wt = dir.join("master");
    assert!(dir.join(".bare").is_dir());
    assert!(!dir.join(".git").exists());
    assert_eq!(fs::read_to_string(wt.join("build/out")).unwrap(), "x");
    assert_eq!(fs::read_to_string(wt.join("master/file")).unwrap(), "m");
    assert_eq!(
        e.git(&wt, &["status", "--porcelain"]),
        "A  staged\n?? .gitignore\n?? master/"
    );
    assert_eq!(e.git(&wt, &["branch", "--show-current"]), "master");
    // The linked worktree moved into its branch dir.
    let feat = dir.join("feat-x");
    assert!(!linked.exists());
    assert_eq!(e.git(&feat, &["branch", "--show-current"]), "feat/x");

    let ls = e.forest(&["ls", "svc", "--json"]).json();
    let mut paths: Vec<&str> = ls["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["dir"].as_str().unwrap())
        .collect();
    paths.sort();
    assert_eq!(paths, ["feat-x", "master"]);
    // origin/HEAD came with the clone, so main stays the default.
    assert_eq!(
        e.git(
            &dir.join(".bare"),
            &["symbolic-ref", "refs/remotes/origin/HEAD"]
        ),
        "refs/remotes/origin/main"
    );

    // Running it again changes nothing.
    let out = e.ok(&["migrate", dir.to_str().unwrap()]);
    assert!(out.contains("already in the layout"), "{out}");
}

#[test]
fn migrate_moves_a_repo_into_the_root() {
    let e = Env::new();
    let src = e.upstream("tool");
    fs::write(src.join("untracked"), "u").unwrap();
    let out = e.ok(&["migrate", src.to_str().unwrap(), "app"]);
    assert!(out.ends_with(e.root.join("app").to_str().unwrap()), "{out}");
    assert!(!src.exists());
    let wt = e.root.join("app/main");
    assert_eq!(fs::read_to_string(wt.join("untracked")).unwrap(), "u");
    assert_eq!(e.git(&wt, &["log", "--format=%s"]), "first");
}

#[test]
fn migrate_bare_repo_with_worktree_inside() {
    let e = Env::new();
    let up = e.upstream("lib");
    let dir = e.root.join("lib.git");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            dir.to_str().unwrap(),
        ],
    );
    e.git(&dir, &["worktree", "add", "-q", "wt", "main"]);
    fs::write(dir.join("wt/dirty"), "d").unwrap();

    e.ok(&["migrate", dir.to_str().unwrap()]);
    let lib = e.root.join("lib");
    assert!(!dir.exists());
    assert!(lib.join(".bare/HEAD").is_file());
    assert!(!lib.join("HEAD").exists());
    assert_eq!(fs::read_to_string(lib.join("main/dirty")).unwrap(), "d");
    assert!(!lib.join("wt").exists());
}

#[test]
fn migrate_bare_repo_without_worktrees_adds_the_default() {
    let e = Env::new();
    let up = e.upstream("lib");
    let dir = e.root.join("lib");
    e.git(
        &e.base,
        &[
            "clone",
            "-q",
            "--bare",
            up.to_str().unwrap(),
            dir.to_str().unwrap(),
        ],
    );
    e.ok(&["migrate", dir.to_str().unwrap()]);
    assert_eq!(
        e.git(&dir.join("main"), &["branch", "--show-current"]),
        "main"
    );
}

#[test]
fn migrate_nearly_laid_out_project() {
    let e = Env::new();
    let wt = e.project("app");
    let dir = e.root.join("app");
    e.git(
        &wt,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/y",
            dir.join("tmp").to_str().unwrap(),
        ],
    );
    // The hand-made variant: `.git` pointing at `.bare`.
    fs::write(dir.join(".git"), "gitdir: ./.bare\n").unwrap();

    e.ok(&["migrate", dir.to_str().unwrap()]);
    assert!(!dir.join(".git").exists());
    assert!(!dir.join("tmp").exists());
    assert_eq!(
        e.git(&dir.join("feat-y"), &["branch", "--show-current"]),
        "feat/y"
    );
}

#[test]
fn migrate_refuses_detached_head_and_linked_worktrees() {
    let e = Env::new();
    let src = e.upstream("x");
    e.git(&src, &["checkout", "-q", "--detach"]);
    let r = e.forest(&["migrate", src.to_str().unwrap()]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(src.join(".git").is_dir());
    assert!(!e.root.join("x").exists());

    let wt = e.project("app");
    let r = e.forest(&["migrate", wt.to_str().unwrap()]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("linked worktree"), "{}", r.stderr);
}
