//! `path --pick`: choose a forest, worktree or repo in a fuzzy picker.

use dialoguer::FuzzySelect;
use dialoguer::console::{Term, style, truncate_str};
use dialoguer::theme::ColorfulTheme;

use crate::cmd::ls::align;
use crate::cmd::path::Located;
use crate::error::{Error, Result, fail};
use crate::layout::{Context, Layout};
use crate::project::all_worktrees;

const FOREST: &str = "🌲";
const REPO: &str = "📦";

/// Every forest, each worktree of a forest with several, then every repo. The
/// picker draws on stderr, so stdout carries only the result.
pub fn pick(layout: &Layout) -> Result<Located> {
    let term = Term::stderr();
    if !term.is_term() {
        return Err(fail!("--pick needs a terminal"));
    }
    let mut entries = entries(layout)?;
    if entries.is_empty() {
        return Err(fail!("nothing to pick: no repos or forests"));
    }
    let rows = entries
        .iter()
        .map(|e| vec![format!("{} {}", e.icon, e.name), e.repos.clone()])
        .collect();
    // A line that wraps breaks the redraw, which counts one line per item. The
    // name comes first and is unique, so cut lines stay apart.
    let width = (term.size().1 as usize).saturating_sub(3).max(10);
    let labels: Vec<String> = align(rows, 0)
        .lines()
        .map(|l| truncate_str(l, width, "…").into_owned())
        .collect();
    // Start on the forest or repo cwd is in.
    let here = match layout.context() {
        Some(Context::Forest(f)) => entries.iter().position(|e| e.name == f),
        Some(Context::Repo(p)) => entries.iter().position(|e| e.icon == REPO && e.name == p),
        None => None,
    };
    let theme = ColorfulTheme {
        prompt_prefix: style(String::new()),
        ..ColorfulTheme::default()
    };
    let picked = FuzzySelect::with_theme(&theme)
        .with_prompt("go to")
        .items(&labels)
        .default(here.unwrap_or(0))
        .report(false)
        .interact_on_opt(&term)
        .map_err(|e| fail!("picker: {e}"))?
        .ok_or(Error::Cancelled)?;
    Ok(entries.swap_remove(picked).located)
}

struct Entry {
    icon: &'static str,
    name: String,
    /// A forest's repos, by name.
    repos: String,
    located: Located,
}

fn entries(layout: &Layout) -> Result<Vec<Entry>> {
    let mut all = all_worktrees(layout)?;
    let mut out = Vec::new();
    for f in layout.forests()? {
        let mut members = Vec::new();
        let mut i = 0;
        while i < all.len() {
            if layout.forest_of(&all[i].1.path).as_deref() == Some(f.as_str()) {
                members.push(all.remove(i));
            } else {
                i += 1;
            }
        }
        let repos: Vec<&str> = members.iter().map(|(p, _)| p.as_str()).collect();
        let repos = repos.join(" ");
        if members.len() == 1 {
            let (project, worktree) = members.remove(0);
            out.push(Entry {
                icon: FOREST,
                name: f.clone(),
                repos,
                located: Located::Worktree {
                    project,
                    worktree,
                    forest: Some(f),
                },
            });
            continue;
        }
        out.push(Entry {
            icon: FOREST,
            name: f.clone(),
            repos,
            located: Located::Forest {
                dir: layout.forest_dir(&f),
                name: f.clone(),
            },
        });
        for (project, worktree) in members {
            out.push(Entry {
                icon: FOREST,
                name: format!("{f}/{project}"),
                repos: String::new(),
                located: Located::Worktree {
                    project,
                    worktree,
                    forest: Some(f.clone()),
                },
            });
        }
    }
    for (project, worktree) in all.into_iter().filter(|(_, w)| w.home) {
        out.push(Entry {
            icon: REPO,
            name: project.clone(),
            repos: String::new(),
            located: Located::Worktree {
                project,
                worktree,
                forest: None,
            },
        });
    }
    Ok(out)
}
