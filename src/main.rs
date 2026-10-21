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
  <project>:<branch>  a worktree, in its project or in a forest
  :<branch>           a worktree of the project you are in
  <forest>            a forest (a word without ':')
  (none)              the worktree or forest you are in";

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
    /// Start a project with no remote
    Init {
        name: String,
        /// Default branch (else git's init.defaultBranch)
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,
    },
    /// Set up a project from a remote (or local) repository
    Clone { url: String, name: Option<String> },
    /// Move an existing repo (normal clone, bare repo, or nearly-laid-out project) into the layout
    Migrate {
        /// The repo's directory
        path: PathBuf,
        /// Project name (default: the directory's name, without `.git`)
        name: Option<String>,
    },
    /// Create a worktree, or a forest with one worktree per project
    #[command(
        override_usage = "\
forest new <PROJECT>:<BRANCH> [--from <REF>]
       forest new :<BRANCH> [--from <REF>]
       forest new <BRANCH> <PROJECT>... [--name <NAME>] [--from <REF>]",
        after_help = "\
Examples:
  forest new api:feat/x             api/feat-x/
  forest new feat/y api web         _forests/feat-y/{api,web}/, all on feat/y
  forest new feat/y api --name y    _forests/y/api/"
    )]
    New {
        /// <project>:<branch> or :<branch> for a worktree; the branch of a forest
        #[arg(value_name = "TARGET")]
        target: String,
        /// Projects of a new forest; given at least one, a forest is made
        #[arg(value_name = "PROJECT")]
        projects: Vec<String>,
        /// The forest's name (default: the branch, with '/' as '-')
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Start a new branch from this ref instead of the default branch
        #[arg(long, value_name = "REF")]
        from: Option<String>,
    },
    /// List projects and forests with their worktrees
    Ls {
        /// A project or forest
        name: Option<String>,
    },
    /// Print the absolute path of a worktree or forest
    #[command(after_help = TARGETS)]
    Path {
        /// <project>:<branch>, :<branch>, or <forest>; default: where you are
        target: Option<String>,
    },
    /// Remove a worktree or forest
    #[command(after_help = TARGETS)]
    Rm {
        /// <project>:<branch>, :<branch>, or <forest>; default: where you are
        target: Option<String>,
        /// Override the safety checks
        #[arg(long)]
        force: bool,
        /// Also delete the local branch, if that is safe
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
            target,
            projects,
            name,
            from,
        } => cmd::new::new(
            &layout,
            &target,
            &projects,
            name.as_deref(),
            from.as_deref(),
        ),
        Command::Ls { name } => cmd::ls::ls(&layout, name.as_deref()),
        Command::Path { target } => cmd::path::path(&layout, target.as_deref()),
        Command::Rm {
            target,
            force,
            delete_branch,
        } => cmd::rm::rm(&layout, target.as_deref(), force, delete_branch),
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
