use crate::{ask, db, index, lang};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server on stdio. `root` is the brain root resolved at startup.
pub fn run_mcp(root: Option<PathBuf>) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "initialize" => reply(
                &mut out,
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "code-reflex", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "ping" => reply(&mut out, id, json!({})),
            "tools/list" => reply(&mut out, id, json!({ "tools": tool_defs() })),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
                match handle_call(root.as_deref(), &params) {
                    Ok(v) => reply(&mut out, id, v),
                    Err(e) => reply_err(&mut out, id, &format!("{e:#}")),
                }
            }
            m if m.starts_with("notifications/") => {}
            _ => {
                if id.is_some() {
                    reply_err(&mut out, id, "method not found");
                }
            }
        }
        out.flush()?;
    }
    Ok(())
}

fn reply(out: &mut impl Write, id: Option<Value>, result: Value) {
    let msg = json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result });
    let _ = writeln!(out, "{msg}");
}

fn reply_err(out: &mut impl Write, id: Option<Value>, message: &str) {
    let msg = json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": { "content": [{ "type": "text", "text": message }], "isError": true }
    });
    let _ = writeln!(out, "{msg}");
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn usize_arg(args: &Value, key: &str, default: usize) -> usize {
    args.get(key).and_then(Value::as_u64).map(|v| v as usize).unwrap_or(default)
}

fn open_brain(root: Option<&Path>) -> Result<(PathBuf, rusqlite::Connection)> {
    let root = root
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .context("no brain root")?;
    let conn = db::open(&root).context("open brain")?;
    Ok((root, conn))
}

fn handle_call(root: Option<&Path>, params: &Value) -> Result<Value> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let limit = usize_arg(&args, "limit", 50).clamp(1, 2000);

    match name {
        "brain_stats" => {
            let (_root, conn) = open_brain(root)?;
            let s = db::stats(&conn)?;
            Ok(text_result(
                serde_json::to_string_pretty(&json!({
                    "files": s.files,
                    "symbols": s.symbols,
                    "issues": s.issues,
                    "symbols_by_kind": s.by_symbol_kind,
                    "issues_by_kind": s.by_issue_kind,
                    "files_by_language": s.by_lang,
                }))
                .unwrap(),
            ))
        }
        "find_symbols" => {
            let (_root, conn) = open_brain(root)?;
            let kind = str_arg(&args, "kind");
            if let Some(k) = &kind {
                if !k.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    anyhow::bail!("invalid kind '{k}'");
                }
            }
            let kinds: Vec<&str> = kind.as_deref().into_iter().collect();
            let rows = db::find_symbols(&conn, &kinds, str_arg(&args, "filter").as_deref(), limit)?;
            Ok(text_result(serde_json::to_string_pretty(&rows)?))
        }
        "list_issues" => {
            let (_root, conn) = open_brain(root)?;
            let kind = str_arg(&args, "kind");
            if let Some(k) = &kind {
                if !k.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    anyhow::bail!("invalid kind '{k}'");
                }
            }
            let kinds: Vec<&str> = kind.as_deref().into_iter().collect();
            let rows = db::list_issues(&conn, &kinds, limit)?;
            Ok(text_result(serde_json::to_string_pretty(&rows)?))
        }
        "outline_file" => {
            let (_root, conn) = open_brain(root)?;
            let path = str_arg(&args, "path").context("argument 'path' is required")?;
            match db::outline(&conn, &path)? {
                Some(o) => Ok(text_result(serde_json::to_string_pretty(&o)?)),
                None => Ok(text_result(format!("file not in brain: {path}"))),
            }
        }
        "search_code" => {
            let (root, _conn) = open_brain(root)?;
            let pattern = str_arg(&args, "pattern").context("argument 'pattern' is required")?;
            let max = usize_arg(&args, "max", 100).clamp(1, 1000);
            let re = regex::Regex::new(&pattern).map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;
            let specs = lang::registry();
            let files = index::collect_files(&root, &specs);
            let mut hits: Vec<(String, usize, String)> = files
                .iter()
                .flat_map(|p| {
                    let rel = index::rel_str(&root, p);
                    let Ok(content) = std::fs::read_to_string(p) else { return Vec::new() };
                    content
                        .lines()
                        .enumerate()
                        .filter(|(_, l)| re.is_match(l))
                        .map(|(i, l)| (rel.clone(), i + 1, l.trim().chars().take(200).collect()))
                        .collect()
                })
                .collect();
            hits.sort();
            hits.truncate(max);
            let text = hits
                .iter()
                .map(|(p, l, t)| format!("{p}:{l}: {t}"))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(text_result(format!("{text}\n{} matches", hits.len())))
        }
        "refresh_brain" => {
            let (root, _conn) = open_brain(root)?;
            let s = index::run_index(&root, false, false)?;
            Ok(text_result(format!(
                "{} files ({} refreshed, {} unchanged, {} removed) in {:?}",
                s.total, s.refreshed, s.unchanged, s.removed, s.elapsed
            )))
        }
        "ask_brain" => {
            let (root, conn) = open_brain(root)?;
            let q = str_arg(&args, "question").context("argument 'question' is required")?;
            let answer = ask::run_ask(&root, &conn, &q)?;
            Ok(text_result(answer))
        }
        other => anyhow::bail!("unknown tool '{other}'"),
    }
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "brain_stats",
            "description": "Overview of the indexed codebase: file/symbol/issue counts by kind and language. Call first to size up a repository.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "find_symbols",
            "description": "Find symbols (classes, structs, enums, functions, methods, constants, fields) by name substring, optionally filtered by kind. Returns kind, name, file:line, signature, parent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "substring matched against symbol names" },
                    "kind": { "type": "string", "description": "one of: class, struct, enum, enum_variant, trait, interface, function, method, constant, field, property, type_alias, module, namespace" },
                    "limit": { "type": "integer", "default": 50 }
                }
            }
        },
        {
            "name": "list_issues",
            "description": "List issues detected in the code: syntax_error, todo, fixme, bug, hack, magic_number, hardcoded_string. Ordered errors first. Great for 'any TODOs / syntax errors / hardcoded values?' questions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "issue kind filter (lowercase, e.g. 'todo')" },
                    "limit": { "type": "integer", "default": 50 }
                }
            }
        },
        {
            "name": "outline_file",
            "description": "Return the full symbol outline plus issues of one file (path or path suffix). Use before editing a file.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "file path or suffix, e.g. src/main.rs" } },
                "required": ["path"]
            }
        },
        {
            "name": "search_code",
            "description": "Regex search over file contents (live, not indexed). Use when the index doesn't cover what you need.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Rust regex syntax" },
                    "max": { "type": "integer", "default": 100 }
                },
                "required": ["pattern"]
            }
        },
        {
            "name": "refresh_brain",
            "description": "Re-index changed files so the brain reflects the latest source. Call after modifying code.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "ask_brain",
            "description": "Natural-language question over the codebase, answered with file:line citations (uses the LLM endpoint configured via KIMI_API_KEY / .env).",
            "inputSchema": {
                "type": "object",
                "properties": { "question": { "type": "string" } },
                "required": ["question"]
            }
        }
    ])
}
