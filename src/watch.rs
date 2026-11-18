use crate::db;
use crate::index;
use crate::lang;
use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn run_watch(root: &std::path::Path, debounce_ms: u64) -> Result<()> {
    let mut conn = db::open(root)?;
    let specs = lang::registry();
    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(ev) = res {
            let _ = tx.send(ev.paths);
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    let debounce = Duration::from_millis(debounce_ms.max(50));
    println!(
        "reflex watch: {} — {} files in brain, refreshing on change (Ctrl-C to stop)",
        root.display(),
        db::file_count(&conn)?
    );

    let mut pending: HashMap<PathBuf, ()> = HashMap::new();
    let mut last_event = Instant::now();
    loop {
        match rx.recv_timeout(debounce) {
            Ok(paths) => {
                for p in paths {
                    if p.is_dir() {
                        continue;
                    }
                    if lang::spec_for_path(&p, &specs).is_none() {
                        continue;
                    }
                    pending.insert(p, ());
                }
                last_event = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending.is_empty() || last_event.elapsed() < debounce {
                    continue;
                }
                let paths: Vec<PathBuf> = pending.drain().map(|(p, _)| p).collect();
                match index::refresh_paths(root, &mut conn, &paths) {
                    Ok(outcomes) => {
                        for o in outcomes {
                            println!(
                                "{:8} {} ({} symbols, {} issues)",
                                o.action, o.rel, o.symbols, o.issues
                            );
                        }
                    }
                    Err(e) => eprintln!("refresh failed: {e:#}"),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

