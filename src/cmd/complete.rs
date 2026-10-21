//! `__complete`: candidates for the word at the cursor, for the shell's completion.
//!
//! It gets the words of the command line up to and including the current one, and
//! prints every candidate for that position, one per line; the shell filters them by
//! what is typed. `:files` asks the shell to complete paths instead.

use std::path::{Path, PathBuf};

use clap::CommandFactory;

use crate::Output;
use crate::cmd::path::{Located, locate};
use crate::error::Result;
use crate::layout::Layout;
use crate::project::{all_worktrees, members};

const FILES: &str = ":files";

/// Never fails: no candidates is the answer to anything that goes wrong.
pub fn complete(layout: &Layout, words: &[String]) -> Output {
    let lines = candidates(layout, words).unwrap_or_default();
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Output::new(text, serde_json::Value::Null)
}

fn candidates(layout: &Layout, words: &[String]) -> Result<Vec<String>> {
    let Some((current, before)) = words.split_last() else {
        return Ok(vec![]);
    };
    let Some((program, rest)) = before.split_first() else {
        return Ok(vec![]);
    };
    let cli = crate::Cli::command();
    let mut root = None;
    // `fcd` takes what `forest path` takes.
    let fcd = Path::new(program).file_name().is_some_and(|n| n == "fcd");
    let mut sub = fcd.then(|| "path".to_string());
    let mut args = Vec::new();
    let mut words = rest.iter();
    while let Some(w) = words.next() {
        if w.starts_with('-') {
            let (flag, inline) = match w.split_once('=') {
                Some((f, v)) => (f, Some(v.to_string())),
                None => (w.as_str(), None),
            };
            let value = match inline {
                Some(v) => Some(v),
                None if takes_value(&cli, sub.as_deref(), flag) => match words.next() {
                    Some(v) => Some(v.clone()),
                    // The cursor is on this flag's value.
                    None => return Ok(value_of(flag)),
                },
                None => None,
            };
            if flag == "--root" {
                root = value.map(PathBuf::from);
            }
        } else if sub.is_none() {
            sub = Some(w.clone());
        } else {
            args.push(w.clone());
        }
    }

    let Some(sub) = sub else {
        if current.starts_with('-') {
            return Ok(flags(&cli));
        }
        return Ok(cli
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(|c| c.get_name().to_string())
            .collect());
    };
    if current.starts_with('-') {
        return Ok(match cli.find_subcommand(&sub) {
            Some(c) if !fcd => flags(c).into_iter().chain(flags(&cli)).collect(),
            _ => vec![],
        });
    }

    // A `--root` on the command line being completed counts, not this one's.
    let own;
    let layout = match root {
        Some(r) => {
            own = Layout::new(Some(r))?;
            &own
        }
        None => layout,
    };
    Ok(match (sub.as_str(), args.len()) {
        ("path" | "rm" | "ls", 0) => targets(layout)?,
        ("path" | "rm", 1) => repos_of(layout, &args[0])?,
        ("mv", 0) => layout.forests()?,
        ("new", 0) => forest_branches(layout)?,
        ("new", _) => {
            let mut repos = layout.projects()?;
            repos.retain(|p| !args[1..].contains(p));
            repos
        }
        ("migrate", 0) => vec![FILES.into()],
        _ => vec![],
    })
}

/// Whether `flag` of the subcommand (or a global one) takes a value.
fn takes_value(cli: &clap::Command, sub: Option<&str>, flag: &str) -> bool {
    let name = flag.trim_start_matches('-');
    let cmds = [Some(cli), sub.and_then(|s| cli.find_subcommand(s))];
    cmds.into_iter().flatten().any(|c| {
        c.get_arguments()
            .any(|a| a.get_long() == Some(name) && a.get_action().takes_values())
    })
}

fn value_of(flag: &str) -> Vec<String> {
    match flag {
        "--root" => vec![FILES.into()],
        _ => vec![],
    }
}

fn flags(cmd: &clap::Command) -> Vec<String> {
    cmd.get_arguments()
        .filter(|a| !a.is_hide_set())
        .filter_map(|a| a.get_long())
        .map(|l| format!("--{l}"))
        .chain(["--help".to_string()])
        .collect()
}

/// Forests, by name and by branch, and repos.
fn targets(layout: &Layout) -> Result<Vec<String>> {
    let mut out = layout.forests()?;
    out.extend(forest_branches(layout)?);
    out.extend(layout.projects()?);
    out.sort();
    out.dedup();
    Ok(out)
}

/// The branches of worktrees in forests.
fn forest_branches(layout: &Layout) -> Result<Vec<String>> {
    let mut out: Vec<String> = all_worktrees(layout)?
        .into_iter()
        .filter(|(_, w)| layout.forest_of(&w.path).is_some())
        .filter_map(|(_, w)| w.branch)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// The repos in the forest `target` names, else every repo.
fn repos_of(layout: &Layout, target: &str) -> Result<Vec<String>> {
    if let Ok(Located::Forest { name, .. }) = locate(layout, Some(target), None) {
        return Ok(members(layout, &name)?
            .into_iter()
            .map(|(p, _)| p)
            .collect());
    }
    layout.projects()
}
