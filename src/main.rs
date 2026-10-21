mod cmd;
mod error;
mod git;
mod layout;
mod project;

use std::path::PathBuf;
use std::process::exit;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde_json::{Map, Value, json};

use crate::error::Result;
use crate::layout::Layout;

const TARGETS: &str = "\
Targets:
  <forest>            a forest, by its name or its branch
  <forest> <repo>     that repo's worktree in the forest
  <branch> <repo>     the repo's worktree on that branch (its home, say)
  <repo>              the repo's home
  (none)              the forest or repo you are in";

/// Worktree & forest manager.
#[derive(Parser)]
#[command(name = "forest", version, after_help = TARGETS)]
struct Cli {
    /// Machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,
    /// Root directory (overrides FOREST_ROOT, default ~/projects)
    #[arg(long, global = true, value_name = "PATH")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a repo with no remote
    Init {
        name: String,
        /// Default branch (else git's init.defaultBranch)
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,
    },
    /// Clone a repo from a remote (or local) repository
    Clone { url: String, name: Option<String> },
    /// Move an existing repo (normal clone, bare repo, or older layout) into the layout
    Migrate {
        /// The repo's directory
        path: PathBuf,
        /// Repo name (default: the directory's name, without `.git`)
        name: Option<String>,
    },
    /// Check out a branch in repos, side by side in its forest
    #[command(after_help = "\
The forest is the one already on the branch, else a new one named after it.

Examples:
  forest new feat/x api                  forests/feat-x/api/
  forest new feat/x web                  adds forests/feat-x/web/
  forest new feat/y api web --as shop    forests/shop/{api,web}/, on feat/y")]
    New {
        branch: String,
        #[arg(value_name = "REPO", required = true)]
        repos: Vec<String>,
        /// Name a new forest (default: the branch, with '/' as '-')
        #[arg(long = "as", value_name = "NAME")]
        alias: Option<String>,
        /// Start a new branch from this ref instead of the default branch
        #[arg(long, value_name = "REF")]
        from: Option<String>,
    },
    /// List repos and forests with their worktrees
    Ls {
        /// A forest (by name or branch) or a repo
        target: Option<String>,
    },
    /// Print the absolute path of a forest or worktree
    ///
    /// A forest with only one worktree prints that worktree.
    #[command(after_help = TARGETS)]
    Path {
        target: Option<String>,
        #[arg(value_name = "REPO", requires = "target")]
        repo: Option<String>,
    },
    /// Remove a forest, or one of its worktrees
    #[command(after_help = TARGETS)]
    Rm {
        target: Option<String>,
        #[arg(value_name = "REPO", requires = "target")]
        repo: Option<String>,
        /// Remove even with uncommitted changes, and delete unmerged branches
        #[arg(long)]
        force: bool,
        /// Also delete the branches, if no commit would be lost
        #[arg(long)]
        delete_branch: bool,
    },
    /// Rename a forest (its dir; branches stay)
    Mv {
        /// A forest, by its name or its branch
        forest: String,
        new_name: String,
    },
    /// Remove every forest whose work is done
    #[command(after_help = "\
A forest is done when none of its worktrees has uncommitted changes, and each
branch is merged into the default branch or its upstream is gone. Ignored files
in it are removed too.")]
    Clean {
        /// Only show what would be removed
        #[arg(long)]
        dry_run: bool,
        /// Also delete the branches, if no commit would be lost
        #[arg(long)]
        delete_branch: bool,
    },
    /// Prune stale worktree metadata and report orphaned dirs
    Prune,
}

/// What a command prints: `text` normally, `json` with `--json`.
pub struct Output {
    pub text: String,
    pub json: Value,
    pub code: i32,
    /// Shown on stderr, or as `"warnings"` with `--json`; they don't fail the command.
    pub warnings: Vec<String>,
}

impl Output {
    pub fn new(text: impl Into<String>, json: Value) -> Self {
        Output {
            text: text.into(),
            json,
            code: 0,
            warnings: Vec::new(),
        }
    }
}

fn run(cli: Cli) -> Result<Output> {
    let layout = Layout::new(cli.root)?;
    match cli.command {
        Command::Init { name, branch } => cmd::setup::init(&layout, &name, branch.as_deref()),
        Command::Clone { url, name } => cmd::setup::clone(&layout, &url, name.as_deref()),
        Command::Migrate { path, name } => cmd::migrate::migrate(&layout, &path, name.as_deref()),
        Command::New {
            branch,
            repos,
            alias,
            from,
        } => cmd::new::new(&layout, &branch, &repos, alias.as_deref(), from.as_deref()),
        Command::Ls { target } => cmd::ls::ls(&layout, target.as_deref()),
        Command::Path { target, repo } => {
            cmd::path::path(&layout, target.as_deref(), repo.as_deref())
        }
        Command::Rm {
            target,
            repo,
            force,
            delete_branch,
        } => cmd::rm::rm(
            &layout,
            target.as_deref(),
            repo.as_deref(),
            force,
            delete_branch,
        ),
        Command::Mv { forest, new_name } => cmd::mv::mv(&layout, &forest, &new_name),
        Command::Clean {
            dry_run,
            delete_branch,
        } => cmd::clean::clean(&layout, dry_run, delete_branch),
        Command::Prune => cmd::prune::prune(&layout),
    }
}

/// Puts `"v":1` first in a JSON object.
fn versioned(value: Value) -> Value {
    let mut out = Map::new();
    out.insert("v".into(), json!(1));
    if let Value::Object(fields) = value {
        out.extend(fields);
    }
    Value::Object(out)
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            // Usage errors exit 1; clap's default of 2 would collide with "refused".
            let code = match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            };
            exit(code);
        }
    };
    let as_json = cli.json;
    match run(cli) {
        Ok(mut out) => {
            if as_json {
                if !out.warnings.is_empty()
                    && let Value::Object(fields) = &mut out.json
                {
                    fields.insert("warnings".into(), json!(out.warnings));
                }
                println!("{}", versioned(out.json));
            } else {
                for w in &out.warnings {
                    eprintln!("forest: warning: {w}");
                }
                print!("{}", out.text);
            }
            exit(out.code);
        }
        Err(e) => {
            if as_json {
                let err = json!({"error": {"code": e.kind(), "message": e.to_string()}});
                println!("{}", versioned(err));
            } else {
                eprintln!("forest: {e}");
            }
            exit(e.exit_code());
        }
    }
}
