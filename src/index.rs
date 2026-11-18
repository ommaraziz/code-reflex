use crate::db;
use crate::extract;
use crate::lang::{self, LangSpec};
use anyhow::Result;
use rayon::prelude::*;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;

pub const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", "dist", "build", "out", "vendor", "bin", "obj", "coverage",
    "__pycache__", ".next", ".nuxt", ".gradle", "Pods", "DerivedData", "tmp",
];

pub fn is_skipped(path: &Path) -> bool {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|s| SKIP_DIRS.contains(&s))
}

pub fn rel_str(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

pub fn hash_bytes(b: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub struct ParsedFile {
    pub rel: String,
    pub lang: String,
    pub hash: String,
    pub size: i64,
    pub mtime: i64,
    pub symbols: Vec<extract::SymbolRec>,
    pub issues: Vec<extract::IssueRec>,
}

pub struct FileOutcome {
    pub rel: String,
    pub action: &'static str,
    pub symbols: usize,
    pub issues: usize,
}

pub struct IndexSummary {
    pub total: usize,
    pub refreshed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub elapsed: Duration,
}

pub fn collect_files(root: &Path, specs: &[LangSpec]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .build();
    for ent in walker.flatten() {
        if ent.file_type().map(|t| t.is_file()) != Some(true) {
            continue;
        }
        let p = ent.path();
        if is_skipped(p) || lang::spec_for_path(p, specs).is_none() {
            continue;
        }
        v.push(p.to_path_buf());
    }
    v
}

pub fn parse_one(root: &Path, path: &Path, specs: &[LangSpec]) -> Option<ParsedFile> {
    let spec = lang::spec_for_path(path, specs)?;
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return None;
    }
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let code = String::from_utf8_lossy(&bytes);
    let out = extract::extract(spec, &code);
    Some(ParsedFile {
        rel: rel_str(root, path),
        lang: spec.name.to_string(),
        hash: hash_bytes(&bytes),
        size: bytes.len() as i64,
        mtime,
        symbols: out.symbols,
        issues: out.issues,
    })
}

fn commit_parsed(conn: &Connection, p: &ParsedFile, existing_id: Option<i64>) -> Result<()> {
    let errors = p.issues.iter().filter(|i| i.severity == "error").count();
    let file_id = match existing_id {
        Some(id) => {
            db::clear_file(conn, id)?;
            db::update_file(conn, id, &p.lang, &p.hash, p.size, p.mtime, errors)?;
            id
        }
        None => db::insert_file(conn, &p.rel, &p.lang, &p.hash, p.size, p.mtime, errors)?,
    };
    db::insert_symbols(conn, file_id, &p.symbols)?;
    db::insert_issues(conn, file_id, &p.issues)?;
    Ok(())
}

fn outcome(p: &ParsedFile, action: &'static str) -> FileOutcome {
    FileOutcome {
        rel: p.rel.clone(),
        action,
        symbols: p.symbols.len(),
        issues: p.issues.len(),
    }
}

pub fn run_index(root: &Path, force: bool, verbose: bool) -> Result<IndexSummary> {
    let start = Instant::now();
    let specs = lang::registry();
    let disk = collect_files(root, &specs);
    let disk_set: HashSet<String> = disk.iter().map(|p| rel_str(root, p)).collect();
    let mut conn = db::open(root)?;
    let existing = db::existing_files(&conn)?;

    // Cheap pre-filter: size/mtime changed? Only those get read and parsed.
    let candidates: Vec<&PathBuf> = disk
        .iter()
        .filter(|p| {
            if force {
                return true;
            }
            let rel = rel_str(root, p);
            match existing.get(&rel) {
                None => true,
                Some((_, _, size, mtime)) => fs::metadata(p)
                    .map(|m| {
                        let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
                        mt != Some(*mtime) || m.len() as i64 != *size
                    })
                    .unwrap_or(true),
            }
        })
        .collect();

    let parsed: Vec<ParsedFile> = candidates
        .par_iter()
        .filter_map(|p| parse_one(root, p, &specs))
        .filter(|pf| {
            // Content identical (e.g. touch): skip the write entirely.
            force
                || existing
                    .get(&pf.rel)
                    .map(|(_, h, _, _)| *h != pf.hash)
                    .unwrap_or(true)
        })
        .collect();

    let tx = conn.transaction()?;
    let mut outcomes = Vec::new();
    for p in &parsed {
        let id = existing.get(&p.rel).map(|(id, _, _, _)| *id);
        commit_parsed(&tx, p, id)?;
        outcomes.push(outcome(p, if id.is_some() { "updated" } else { "added" }));
    }
    let removed = db::prune(&tx, &disk_set)?;
    db::set_meta(&tx, "last_index_at", &format!("{:?}", Instant::now() - start))?;
    db::set_meta(&tx, "last_index_duration_ms", &format!("{}", (Instant::now() - start).as_millis()))?;
    tx.commit()?;

    if verbose {
        for o in &outcomes {
            println!(
                "{:8} {} ({} symbols, {} issues)",
                o.action, o.rel, o.symbols, o.issues
            );
        }
        if removed > 0 {
            println!("{removed} files removed from brain");
        }
    }

    let refreshed = outcomes.len();
    Ok(IndexSummary {
        total: disk.len(),
        refreshed,
        unchanged: disk.len().saturating_sub(refreshed),
        removed,
        elapsed: start.elapsed(),
    })
}

/// Refresh a small set of paths (used by watch mode). Returns outcomes.
pub fn refresh_paths(root: &Path, conn: &mut Connection, paths: &[PathBuf]) -> Result<Vec<FileOutcome>> {
    let specs = lang::registry();
    let existing = db::existing_files(conn)?;
    let mut outcomes = Vec::new();
    let tx = conn.transaction()?;
    for path in paths {
        let rel = rel_str(root, path);
        if !path.exists() {
            if db::remove_file(&tx, &rel)? {
                outcomes.push(FileOutcome { rel, action: "removed", symbols: 0, issues: 0 });
            }
            continue;
        }
        if is_skipped(path) {
            continue;
        }
        let Some(pf) = parse_one(root, path, &specs) else { continue };
        let id = existing.get(&pf.rel).map(|(id, _, _, _)| *id);
        commit_parsed(&tx, &pf, id)?;
        outcomes.push(outcome(&pf, if id.is_some() { "updated" } else { "added" }));
    }
    tx.commit()?;
    Ok(outcomes)
}
