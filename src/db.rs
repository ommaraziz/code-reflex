use crate::extract::{IssueRec, SymbolRec};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Serialize, Clone)]
pub struct SymbolRow {
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct IssueRow {
    pub severity: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

#[derive(Serialize)]
pub struct Outline {
    pub file: String,
    pub lang: String,
    pub symbols: Vec<SymbolRow>,
    pub issues: Vec<IssueRow>,
}

pub struct Stats {
    pub files: i64,
    pub symbols: i64,
    pub issues: i64,
    pub by_symbol_kind: Vec<(String, i64)>,
    pub by_issue_kind: Vec<(String, i64)>,
    pub by_lang: Vec<(String, i64)>,
    pub top_issue_files: Vec<(String, i64)>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  lang TEXT NOT NULL,
  hash TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  error_count INTEGER NOT NULL DEFAULT 0,
  indexed_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS symbols (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  signature TEXT,
  line INTEGER NOT NULL,
  col INTEGER NOT NULL,
  end_line INTEGER NOT NULL,
  parent TEXT
);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
CREATE TABLE IF NOT EXISTS issues (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL,
  line INTEGER NOT NULL,
  col INTEGER NOT NULL,
  message TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues(kind);
CREATE INDEX IF NOT EXISTS idx_issues_file ON issues(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(name, signature);
";

pub fn open(root: &Path) -> Result<Connection> {
    let dir = root.join(".reflex");
    fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("brain.db")).context("open .reflex/brain.db")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
        .optional()?)
}

/// path -> (id, hash, size, mtime)
pub fn existing_files(conn: &Connection) -> Result<HashMap<String, (i64, String, i64, i64)>> {
    let mut stmt = conn.prepare("SELECT id, path, hash, size, mtime FROM files")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
    })?;
    let mut m = HashMap::new();
    for r in rows {
        let (id, p, h, size, mtime) = r?;
        m.insert(p, (id, h, size, mtime));
    }
    Ok(m)
}

pub fn insert_file(conn: &Connection, path: &str, lang: &str, hash: &str, size: i64, mtime: i64, error_count: usize) -> Result<i64> {
    conn.execute(
        "INSERT INTO files (path, lang, hash, size, mtime, error_count, indexed_at) VALUES (?1,?2,?3,?4,?5,?6, datetime('now'))",
        params![path, lang, hash, size, mtime, error_count as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_file(conn: &Connection, id: i64, lang: &str, hash: &str, size: i64, mtime: i64, error_count: usize) -> Result<()> {
    conn.execute(
        "UPDATE files SET lang=?2, hash=?3, size=?4, mtime=?5, error_count=?6, indexed_at=datetime('now') WHERE id=?1",
        params![id, lang, hash, size, mtime, error_count as i64],
    )?;
    Ok(())
}

pub fn clear_file(conn: &Connection, file_id: i64) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM symbols WHERE file_id = ?1")?;
        let rows = stmt.query_map(params![file_id], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for id in ids {
        conn.execute("DELETE FROM symbols_fts WHERE rowid = ?1", params![id])?;
    }
    conn.execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])?;
    conn.execute("DELETE FROM issues WHERE file_id = ?1", params![file_id])?;
    Ok(())
}

pub fn remove_file(conn: &Connection, path: &str) -> Result<bool> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM files WHERE path = ?1", params![path], |r| r.get(0))
        .optional()?;
    if let Some(id) = id {
        clear_file(conn, id)?;
        conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn insert_symbols(conn: &Connection, file_id: i64, symbols: &[SymbolRec]) -> Result<()> {
    let mut st = conn.prepare(
        "INSERT INTO symbols (file_id, kind, name, signature, line, col, end_line, parent) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
    )?;
    let mut ft = conn.prepare("INSERT INTO symbols_fts (rowid, name, signature) VALUES (?1,?2,?3)")?;
    for s in symbols {
        st.execute(params![
            file_id,
            s.kind.as_str(),
            s.name,
            s.signature,
            s.line as i64,
            s.col as i64,
            s.end_line as i64,
            s.parent,
        ])?;
        let id = conn.last_insert_rowid();
        ft.execute(params![id, s.name, s.signature])?;
    }
    Ok(())
}

pub fn insert_issues(conn: &Connection, file_id: i64, issues: &[IssueRec]) -> Result<()> {
    let mut st = conn.prepare(
        "INSERT INTO issues (file_id, kind, severity, line, col, message) VALUES (?1,?2,?3,?4,?5,?6)",
    )?;
    for i in issues {
        st.execute(params![file_id, i.kind, i.severity, i.line as i64, i.col as i64, i.message])?;
    }
    Ok(())
}

/// Delete files that are no longer on disk. Returns removed count.
pub fn prune(conn: &Connection, disk: &HashSet<String>) -> Result<usize> {
    let known = existing_files(conn)?;
    let mut n = 0;
    for (p, (id, _, _, _)) in known {
        if !disk.contains(&p) {
            clear_file(conn, id)?;
            conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
            n += 1;
        }
    }
    Ok(n)
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").replace('\'', "''")
}

fn sql_str(s: &str) -> String {
    s.replace('\'', "''")
}

pub fn find_symbols(conn: &Connection, kinds: &[&str], filter: Option<&str>, limit: usize) -> Result<Vec<SymbolRow>> {
    let mut sql = String::from(
        "SELECT s.kind, s.name, s.signature, f.path, s.line, s.end_line, s.parent \
         FROM symbols s JOIN files f ON f.id = s.file_id WHERE 1=1",
    );
    if !kinds.is_empty() {
        let list = kinds.iter().map(|k| format!("'{}'", sql_str(k))).collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND s.kind IN ({list})"));
    }
    if let Some(f) = filter {
        let esc = like_escape(f);
        sql.push_str(&format!(" AND s.name LIKE '%{esc}%' ESCAPE '\\'"));
    }
    sql.push_str(&format!(" ORDER BY f.path, s.line LIMIT {}", limit as i64));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(SymbolRow {
            kind: r.get(0)?,
            name: r.get(1)?,
            signature: r.get(2)?,
            file: r.get(3)?,
            line: r.get::<_, i64>(4)? as usize,
            end_line: r.get::<_, i64>(5)? as usize,
            parent: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn list_issues(conn: &Connection, kinds: &[&str], limit: usize) -> Result<Vec<IssueRow>> {
    let mut sql = String::from(
        "SELECT i.severity, i.kind, f.path, i.line, i.col, i.message \
         FROM issues i JOIN files f ON f.id = i.file_id WHERE 1=1",
    );
    if !kinds.is_empty() {
        let list = kinds.iter().map(|k| format!("'{}'", sql_str(k))).collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND i.kind IN ({list})"));
    }
    sql.push_str(&format!(
        " ORDER BY CASE i.severity WHEN 'error' THEN 0 WHEN 'warn' THEN 1 ELSE 2 END, f.path, i.line LIMIT {}",
        limit as i64
    ));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(IssueRow {
            severity: r.get(0)?,
            kind: r.get(1)?,
            file: r.get(2)?,
            line: r.get::<_, i64>(3)? as usize,
            col: r.get::<_, i64>(4)? as usize,
            message: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn outline(conn: &Connection, suffix: &str) -> Result<Option<Outline>> {
    let esc = like_escape(suffix);
    let file: Option<(i64, String, String)> = conn
        .query_row(
            &format!(
                "SELECT id, path, lang FROM files WHERE path = '{}' OR path LIKE '%{esc}' ESCAPE '\\' ORDER BY LENGTH(path) LIMIT 1",
                sql_str(suffix)
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some(file) = file else { return Ok(None) };
    let (file_id, path, lang) = file;
    let mut stmt = conn.prepare(
        "SELECT kind, name, signature, line, end_line, parent FROM symbols WHERE file_id = ?1 ORDER BY line",
    )?;
    let symbols = stmt
        .query_map(params![file_id], |r| {
            Ok(SymbolRow {
                kind: r.get(0)?,
                name: r.get(1)?,
                signature: r.get(2)?,
                file: path.clone(),
                line: r.get::<_, i64>(3)? as usize,
                end_line: r.get::<_, i64>(4)? as usize,
                parent: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(
        "SELECT severity, kind, line, col, message FROM issues WHERE file_id = ?1 ORDER BY line",
    )?;
    let issues = stmt
        .query_map(params![file_id], |r| {
            Ok(IssueRow {
                severity: r.get(0)?,
                kind: r.get(1)?,
                file: path.clone(),
                line: r.get::<_, i64>(2)? as usize,
                col: r.get::<_, i64>(3)? as usize,
                message: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(Outline { file: path, lang, symbols, issues }))
}

pub fn search_fts(conn: &Connection, match_expr: &str, limit: usize) -> Result<Vec<SymbolRow>> {
    let sql = "SELECT s.kind, s.name, s.signature, f.path, s.line, s.end_line, s.parent \
               FROM symbols_fts fts JOIN symbols s ON s.id = fts.rowid JOIN files f ON f.id = s.file_id \
               WHERE symbols_fts MATCH ?1 ORDER BY rank LIMIT ?2";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![match_expr, limit as i64], |r| {
        Ok(SymbolRow {
            kind: r.get(0)?,
            name: r.get(1)?,
            signature: r.get(2)?,
            file: r.get(3)?,
            line: r.get::<_, i64>(4)? as usize,
            end_line: r.get::<_, i64>(5)? as usize,
            parent: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn count_by(conn: &Connection, sql: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let files = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let symbols = conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
    let issues = conn.query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))?;
    Ok(Stats {
        files,
        symbols,
        issues,
        by_symbol_kind: count_by(conn, "SELECT kind, COUNT(*) FROM symbols GROUP BY kind ORDER BY 2 DESC")?,
        by_issue_kind: count_by(conn, "SELECT kind, COUNT(*) FROM issues GROUP BY kind ORDER BY 2 DESC")?,
        by_lang: count_by(conn, "SELECT lang, COUNT(*) FROM files GROUP BY lang ORDER BY 2 DESC")?,
        top_issue_files: count_by(
            conn,
            "SELECT f.path, COUNT(*) FROM issues i JOIN files f ON f.id = i.file_id GROUP BY f.path ORDER BY 2 DESC LIMIT 5",
        )?,
    })
}

pub fn file_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?)
}
