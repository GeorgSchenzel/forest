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

/// Worktree & workforest manager.
#[derive(Parser)]
#[command(name = "forest", version)]
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
    /// Create a forest, or change its project list
    Define {
        /// Forest name
        name: String,
        /// Projects of a new forest
        projects: Vec<String>,
        /// Add projects to an existing forest
        #[arg(long, value_name = "PROJECT", num_args = 1..)]
        add: Vec<String>,
        /// Remove projects from an existing forest
        #[arg(long, value_name = "PROJECT", num_args = 1..)]
        remove: Vec<String>,
    },
    /// Create a worktree (project) or workforest (forest)
    New {
        /// <name>:<branch>, or a branch in the current project/forest
        target: String,
        /// Start a new branch from this ref instead of the default branch
        #[arg(long, value_name = "REF")]
        from: Option<String>,
    },
    /// List projects, forests, worktrees and workforests
    Ls {
        /// A project or forest
        name: Option<String>,
    },
    /// Print the absolute path of a worktree or workforest
    Path {
        /// <name>:<branch>, a branch in the current context, or cwd's worktree/workforest
        target: Option<String>,
    },
    /// Remove a worktree or workforest
    Rm {
        /// <name>:<branch>, a branch in the current context, or cwd's worktree/workforest
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
}

impl Output {
    pub fn new(text: impl Into<String>, json: Value) -> Self {
        Output {
            text: text.into(),
            json,
            code: 0,
        }
    }
}

fn run(cli: Cli) -> Result<Output> {
    let layout = Layout::new(cli.root)?;
    match cli.command {
        Command::Init { name, branch } => cmd::setup::init(&layout, &name, branch.as_deref()),
        Command::Clone { url, name } => cmd::setup::clone(&layout, &url, name.as_deref()),
        Command::Migrate { path, name } => cmd::migrate::migrate(&layout, &path, name.as_deref()),
        Command::Define {
            name,
            projects,
            add,
            remove,
        } => cmd::define::define(&layout, &name, &projects, &add, &remove),
        Command::New { target, from } => cmd::new::new(&layout, &target, from.as_deref()),
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
        Ok(out) => {
            if as_json {
                println!("{}", versioned(out.json));
            } else {
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
