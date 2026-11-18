use crate::db::{self, IssueRow, SymbolRow};
use crate::index;
use crate::lang;
use anyhow::Result;
use rayon::prelude::*;
use rusqlite::Connection;

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn pad(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    format!("{t:<w$}")
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(none)");
        return;
    }
    let caps = [10usize, 38, 44, 34, 60, 60];
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(c.chars().count().min(80));
            }
        }
    }
    for (i, w) in widths.iter_mut().enumerate() {
        let cap = caps.get(i).copied().unwrap_or(40);
        *w = (*w).min(cap).max(headers.get(i).map(|h| h.len()).unwrap_or(4));
    }
    let render = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
    };
    println!("{}", render(&headers.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
    println!("{}", widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "));
    for r in rows {
        println!("{}", render(r));
    }
}

fn loc(r: &SymbolRow) -> String {
    format!("{}:{}", r.file, r.line)
}

fn loc_i(r: &IssueRow) -> String {
    format!("{}:{}", r.file, r.line)
}

pub fn print_symbols(rows: &[SymbolRow], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(rows)?);
        return Ok(());
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.kind.clone(),
                r.name.clone(),
                loc(r),
                r.parent.clone().unwrap_or_default(),
                r.signature.clone().unwrap_or_default(),
            ]
        })
        .collect();
    print_table(&["KIND", "NAME", "LOCATION", "PARENT", "SIGNATURE"], &table);
    println!("{} symbols", rows.len());
    Ok(())
}

pub fn print_issues(rows: &[IssueRow], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(rows)?);
        return Ok(());
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| vec![r.severity.clone(), r.kind.clone(), loc_i(r), r.message.clone()])
        .collect();
    print_table(&["SEVERITY", "KIND", "LOCATION", "MESSAGE"], &table);
    println!("{} issues", rows.len());
    Ok(())
}

pub fn cmd_symbols(conn: &Connection, kinds: &[&str], filter: Option<&str>, limit: usize, json: bool) -> Result<()> {
    let rows = db::find_symbols(conn, kinds, filter, limit)?;
    print_symbols(&rows, json)
}

pub fn cmd_issues(conn: &Connection, kinds: &[&str], limit: usize, json: bool) -> Result<()> {
    let rows = db::list_issues(conn, kinds, limit)?;
    print_issues(&rows, json)
}

pub fn cmd_outline(conn: &Connection, path: &str, json: bool) -> Result<()> {
    let Some(o) = db::outline(conn, path)? else {
        println!("file not in brain: {path}");
        return Ok(());
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&o)?);
        return Ok(());
    }
    let errors = o.issues.iter().filter(|i| i.severity == "error").count();
    println!(
        "{} — {}, {} symbols, {} issues ({} errors)",
        o.file,
        o.lang,
        o.symbols.len(),
        o.issues.len(),
        errors
    );
    if !o.symbols.is_empty() {
        println!();
        let table: Vec<Vec<String>> = o
            .symbols
            .iter()
            .map(|s| {
                vec![
                    s.line.to_string(),
                    s.kind.clone(),
                    s.name.clone(),
                    s.parent.clone().unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["LINE", "KIND", "NAME", "PARENT"], &table);
    }
    if !o.issues.is_empty() {
        println!();
        let table: Vec<Vec<String>> = o
            .issues
            .iter()
            .map(|i| vec![i.line.to_string(), i.severity.clone(), i.kind.clone(), i.message.clone()])
            .collect();
        print_table(&["LINE", "SEVERITY", "KIND", "MESSAGE"], &table);
    }
    Ok(())
}

pub fn cmd_stats(conn: &Connection) -> Result<()> {
    let s = db::stats(conn)?;
    println!("files    {}", s.files);
    println!("symbols  {}", s.symbols);
    println!("issues   {}", s.issues);
    if let Some(t) = db::get_meta(conn, "last_index_at")? {
        println!("last full index took {t}");
    }
    if !s.by_lang.is_empty() {
        println!();
        println!("by language:");
        for (k, n) in &s.by_lang {
            println!("  {n:6}  {k}");
        }
    }
    if !s.by_symbol_kind.is_empty() {
        println!();
        println!("symbols by kind:");
        for (k, n) in &s.by_symbol_kind {
            println!("  {n:6}  {k}");
        }
    }
    if !s.by_issue_kind.is_empty() {
        println!();
        println!("issues by kind:");
        for (k, n) in &s.by_issue_kind {
            println!("  {n:6}  {k}");
        }
    }
    if !s.top_issue_files.is_empty() {
        println!();
        println!("files with most issues:");
        for (p, n) in &s.top_issue_files {
            println!("  {n:6}  {p}");
        }
    }
    Ok(())
}

pub fn cmd_grep(root: &std::path::Path, pattern: &str, max: usize) -> Result<()> {
    let re = regex::Regex::new(pattern).map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;
    let specs = lang::registry();
    let files = index::collect_files(root, &specs);
    let mut hits: Vec<(String, usize, String)> = files
        .par_iter()
        .flat_map(|p| {
            let rel = index::rel_str(root, p);
            let Ok(content) = std::fs::read_to_string(p) else { return Vec::new() };
            content
                .lines()
                .enumerate()
                .filter(|(_, l)| re.is_match(l))
                .map(|(i, l)| (rel.clone(), i + 1, l.trim().chars().take(160).collect()))
                .collect()
        })
        .collect();
    hits.sort();
    hits.truncate(max);
    for (p, line, text) in &hits {
        println!("{p}:{line}: {text}");
    }
    println!("{} matches", hits.len());
    Ok(())
}
