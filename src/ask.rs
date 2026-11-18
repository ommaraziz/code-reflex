use crate::db;
use anyhow::{bail, Result};
use rusqlite::Connection;
use std::env;
use std::path::{Path, PathBuf};

const DEFAULT_BASE: &str = "https://api.moonshot.ai/v1";
const DEFAULT_MODEL: &str = "kimi-k2-0905-preview";

fn keywords(question: &str, take: usize) -> Vec<String> {
    question
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|t| t.trim_matches(|c: char| c.is_numeric()).to_string())
        .filter(|t| t.chars().count() >= 3)
        .take(take)
        .collect()
}

/// Read up to `max_lines` lines of a symbol's body from disk.
fn read_snippet(root: &Path, file: &str, line: usize, end_line: usize) -> Option<String> {
    const MAX_LINES: usize = 50;
    let content = std::fs::read_to_string(root.join(file)).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = line.saturating_sub(1);
    let end = (end_line.min(start + MAX_LINES)).min(lines.len());
    if start >= end {
        return None;
    }
    Some(lines[start..end].join("\n"))
}

/// Environment variable takes precedence; .env.local / .env at the brain root are
/// the fallback, then the user-level config at ~/.config/reflex/.env.
fn config_value(root: &Path, key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| dotenv_value(root, key))
        .or_else(|| {
            dirs_home()
                .map(|h| h.join(".config/reflex/.env"))
                .and_then(|p| dotenv_value_at(&p, key))
        })
}

fn dirs_home() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn dotenv_value_at(path: &Path, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Read KEY=VALUE lines from .env.local / .env at the repo root.
fn dotenv_value(root: &Path, key: &str) -> Option<String> {
    for name in [".env.local", ".env"] {
        if let Some(v) = dotenv_value_at(&root.join(name), key) {
            return Some(v);
        }
    }
    None
}

pub fn run_ask(root: &Path, conn: &Connection, question: &str) -> Result<()> {
    let key = config_value(root, "KIMI_API_KEY").ok_or_else(|| {
        anyhow::anyhow!(
            "KIMI_API_KEY is not set.\n\
             The brain index works offline, but `ask` needs a Kimi API token, either:\n\
               export KIMI_API_KEY=sk-...\n\
             or a line in .env.local (or .env) at the repo root, or in ~/.config/reflex/.env\n\
             (one key setup for all repos):\n\
               KIMI_API_KEY=sk-...\n\
             Optional overrides: KIMI_BASE_URL (default {DEFAULT_BASE}), KIMI_MODEL (default {DEFAULT_MODEL}),\n\
             KIMI_API_STYLE (openai or anthropic; anthropic for Kimi-for-Coding endpoints)."
        )
    })?;
    let base = config_value(root, "KIMI_BASE_URL").unwrap_or_else(|| DEFAULT_BASE.into());
    let model = config_value(root, "KIMI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into());

    let mut context = String::new();

    let toks = keywords(question, 8);
    if !toks.is_empty() {
        let match_expr = toks
            .iter()
            .map(|t| format!("\"{}\"", t.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect::<String>()))
            .collect::<Vec<_>>()
            .join(" OR ");
        let hits = db::search_fts(conn, &match_expr, 25)?;
        if !hits.is_empty() {
            context.push_str("## Relevant symbols from the index\n");
            for h in &hits {
                context.push_str(&format!(
                    "- {} `{}` at {}:{}{}\n",
                    h.kind,
                    h.name,
                    h.file,
                    h.line,
                    h.signature
                        .as_deref()
                        .map(|s| format!(" — {s}"))
                        .unwrap_or_default()
                ));
            }
            context.push('\n');
            // Include the bodies of the top hits so the model can reason
            // about implementation, not just names.
            context.push_str("## Top relevant code\n");
            for h in hits.iter().take(8) {
                if let Some(snippet) = read_snippet(root, &h.file, h.line, h.end_line) {
                    context.push_str(&format!("### {}:{} — `{}`\n```\n{}\n```\n", h.file, h.line, h.name, snippet));
                }
            }
            context.push('\n');
        }
    }

    let issues = db::list_issues(conn, &["syntax_error", "todo", "fixme", "bug"], 30)?;
    if !issues.is_empty() {
        context.push_str("## Known issues (syntax errors, TODO/FIXME, BUG)\n");
        for i in &issues {
            context.push_str(&format!("- [{}] {} at {}:{} — {}\n", i.severity, i.kind, i.file, i.line, i.message));
        }
        context.push('\n');
    }

    let s = db::stats(conn)?;
    context.push_str(&format!(
        "## Brain stats\n{} files, {} symbols, {} issues\n",
        s.files, s.symbols, s.issues
    ));

    let system = format!(
        "You are the brain of the codebase at {}. Answer the user's question using ONLY the \
         indexed context below. Cite exact file:line for every claim. If the context is not \
         sufficient to answer, say precisely what information is missing instead of guessing.",
        root.display()
    );
    let user = format!("{context}\n\n## Question\n{question}");

    let style = config_value(root, "KIMI_API_STYLE").unwrap_or_else(|| "openai".to_string());
    let answer: String = if style == "anthropic" {
        // Anthropic Messages API (used by Kimi-for-Coding style endpoints):
        // POST {base}/v1/messages with x-api-key + anthropic-version headers.
        let b = base.trim_end_matches('/');
        let url = if b.ends_with("/v1") { format!("{b}/messages") } else { format!("{b}/v1/messages") };
        let resp = ureq::post(&url)
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01")
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(90))
            .send_json(serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            }));
        let body = match resp {
            Ok(r) => r.into_json::<serde_json::Value>()?,
            Err(ureq::Error::Status(code, r)) => {
                bail!("Kimi API returned {code} from {url}: {}", r.into_string().unwrap_or_default())
            }
            Err(e) => bail!("could not reach Kimi API at {url}: {e}"),
        };
        // The endpoint may prefix thinking blocks before the text block.
        body.pointer("/content")
            .and_then(|c| c.as_array())
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .and_then(|b| b.get("text"))
                    .and_then(|t| t.as_str())
            })
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("unexpected Kimi API response: {body}"))?
    } else {
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let resp = ureq::post(&url)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(90))
            .send_json(serde_json::json!({
                "model": model,
                "temperature": 0.2,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            }));
        let body = match resp {
            Ok(r) => r.into_json::<serde_json::Value>()?,
            Err(ureq::Error::Status(code, r)) => {
                bail!("Kimi API returned {code} from {url}: {}", r.into_string().unwrap_or_default())
            }
            Err(e) => bail!("could not reach Kimi API at {url}: {e}"),
        };
        body.pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("unexpected Kimi API response: {body}"))?
    };
    println!("{answer}");
    Ok(())
}
