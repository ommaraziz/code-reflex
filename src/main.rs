mod ask;
mod cli;
mod db;
mod extract;
mod index;
mod lang;
mod mcp;
mod query;
mod watch;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{Cli, Commands, FindArgs};
use std::env;
use std::path::PathBuf;

fn find_root(or_cwd: bool) -> Result<PathBuf> {
    let cwd = env::current_dir()?;
    for anc in cwd.ancestors() {
        if anc.join(".reflex/brain.db").exists() {
            return Ok(anc.to_path_buf());
        }
    }
    if or_cwd {
        Ok(cwd)
    } else {
        bail!("no brain found. Run `reflex init` (or `reflex index`) at your project root first.")
    }
}

fn valid_kind(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

fn kind_command(args: FindArgs, kinds: &[&str]) -> Result<()> {
    let root = find_root(false)?;
    let conn = db::open(&root)?;
    query::cmd_symbols(&conn, kinds, args.filter.as_deref(), args.limit, args.json)
}

fn main() -> Result<()> {
    // Die silently on SIGPIPE like a normal Unix filter instead of panicking
    // when the consumer of our output (head, less, ...) closes the pipe.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match cli.command {
        Commands::Init => {
            let root = env::current_dir()?;
            let brain = root.join(".reflex/brain.db");
            if brain.exists() {
                println!("brain already initialized: {}", brain.display());
            } else {
                let _ = db::open(&root)?;
                println!("brain initialized: {}", brain.display());
                println!("tip: add `.reflex/` to your .gitignore, then run `reflex index`");
            }
            Ok(())
        }
        Commands::Index { force, verbose } => {
            let root = find_root(true)?;
            let s = index::run_index(&root, force, verbose)?;
            println!(
                "{} files ({} refreshed, {} unchanged, {} removed) in {:?}",
                s.total, s.refreshed, s.unchanged, s.removed, s.elapsed
            );
            let conn = db::open(&root)?;
            let st = db::stats(&conn)?;
            println!("brain holds {} symbols and {} issues across {} files", st.symbols, st.issues, st.files);
            Ok(())
        }
        Commands::Watch { debounce } => {
            let root = find_root(false)?;
            watch::run_watch(&root, debounce)
        }
        Commands::Stats => {
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_stats(&conn)
        }
        Commands::Symbols { filter, kind, limit, json } => {
            let kinds: Vec<&str> = match &kind {
                Some(k) => {
                    if !valid_kind(k) {
                        bail!("invalid --kind '{k}': lowercase letters and underscores only")
                    }
                    vec![k.as_str()]
                }
                None => vec![],
            };
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_symbols(&conn, &kinds, filter.as_deref(), limit, json)
        }
        Commands::Classes(a) => kind_command(a, &["class"]),
        Commands::Structs(a) => kind_command(a, &["struct"]),
        Commands::Enums(a) => kind_command(a, &["enum", "enum_variant"]),
        Commands::Fns(a) => kind_command(a, &["function", "method"]),
        Commands::Traits(a) => kind_command(a, &["trait", "interface"]),
        Commands::Consts(a) => kind_command(a, &["constant"]),
        Commands::Outline { path, json } => {
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_outline(&conn, &path, json)
        }
        Commands::Issues { kind, limit, json } => {
            let kinds: Vec<&str> = match &kind {
                Some(k) => {
                    if !valid_kind(k) {
                        bail!("invalid --kind '{k}'")
                    }
                    vec![k.as_str()]
                }
                None => vec![],
            };
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_issues(&conn, &kinds, limit, json)
        }
        Commands::Errors { limit, json } => {
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_issues(&conn, &["syntax_error"], limit, json)
        }
        Commands::Hardcoded { strings_only, numbers_only, limit, json } => {
            let kinds: &[&str] = if strings_only {
                &["hardcoded_string"]
            } else if numbers_only {
                &["magic_number"]
            } else {
                &["hardcoded_string", "magic_number"]
            };
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            query::cmd_issues(&conn, kinds, limit, json)
        }
        Commands::Grep { pattern, max } => {
            let root = find_root(false)?;
            query::cmd_grep(&root, &pattern, max)
        }
        Commands::Ask { question } => {
            let root = find_root(false)?;
            let conn = db::open(&root)?;
            let q = question.join(" ");
            if q.trim().is_empty() {
                bail!("usage: reflex ask \"your question\"")
            }
            let answer = ask::run_ask(&root, &conn, &q)?;
            println!("{answer}");
            Ok(())
        }
        Commands::Mcp => {
            let root = find_root(true).ok();
            mcp::run_mcp(root)
        }
    }
}
