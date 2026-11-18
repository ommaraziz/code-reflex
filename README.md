# code-reflex

**The persistent brain of your codebase.** Index once — then ask where any class, enum, function, constant, hardcoded string, magic number, TODO, or syntax error lives, and get the answer from the index in milliseconds, not from a slow re-scan.

```
$ reflex enums Status
KIND        NAME       LOCATION
enum        Status     src/order.rs:12
enum_vari…  Pending    src/order.rs:13
enum_vari…  Paid       src/order.rs:14

$ reflex errors
SEVERITY  KIND          LOCATION              MESSAGE
error     syntax_error  src/parser.rs:88      parse error: unexpected token

$ reflex ask "how does incremental re-index decide what to re-parse?"
collect_files builds the candidate set by walking the root... (src/index.rs:61)
```

## Why

Every code question ("where is that enum?", "did I leave any TODOs?", "is there a syntax error anywhere?") currently costs you a slow grep or a full-project search. `code-reflex` pays that cost once: it parses the whole repo into a SQLite brain, keeps it fresh with surgical, per-file refreshes, and answers every question from the index. The optional `ask` command layers an LLM on top, so you can question the code in plain English and get answers with exact `file:line` citations.

All index and query features are **fully offline** — the LLM is only used when you explicitly run `reflex ask`.

## Features

- **Symbol index** — classes, structs, enums (+variants), traits/interfaces, functions/methods, constants, fields, with signatures, parents, and `file:line` locations
- **Issue radar** — syntax errors (tree-sitter `ERROR`/`MISSING` nodes), TODO/FIXME/HACK/XXX/BUG comments, magic numbers, hardcoded strings
- **Incremental by design** — content-hash gating means `reflex index` only re-parses changed files; a full no-op re-check of a 756-file repo takes ~37ms
- **Watch mode** — `reflex watch` refreshes single files within milliseconds of saving
- **Fast search** — SQLite FTS5 over every symbol + live regex `grep`
- **Ask anything** — natural-language Q&A over your code via any OpenAI-compatible or Anthropic-compatible endpoint (Kimi, OpenAI, local llama.cpp/Ollama proxies, …)
- **12 languages** — Rust, Python, JavaScript/JSX, TypeScript/TSX, Go, Java, C, C++, C#, Bash, JSON
- **Scriptable** — `--json` output on every list command; zero config beyond one env file

## Install

```bash
git clone https://github.com/ommaraziz/code-reflex.git
cd code-reflex
cargo build --release
cp target/release/reflex ~/.local/bin   # or anywhere on your PATH
```

Requires a Rust toolchain (1.90+). No runtime dependencies — one static binary, your brain is a single portable `brain.db`.

## Quick start

```bash
cd your-project
reflex init        # once: creates .reflex/
reflex index       # full scan (later runs only touch changed files)
reflex watch       # optional: keep it fresh automatically
```

Add `.reflex/` to your `.gitignore` (the brain is reproducible from source).

## Commands

| Command | What it answers |
|---|---|
| `reflex index [--force] [--verbose]` | Refresh the brain (incremental) |
| `reflex watch [--debounce ms]` | Auto-refresh on every save |
| `reflex stats` | Brain statistics and hotspots |
| `reflex symbols [FILTER] [--kind k] [--json]` | Find any symbol |
| `reflex classes / structs / enums / fns / traits / consts [FILTER]` | Browse by kind |
| `reflex outline <file>` | Map of one file: symbols + issues |
| `reflex issues [--kind k]` | All issues, errors first |
| `reflex errors` | Syntax errors only |
| `reflex hardcoded [--strings-only \| --numbers-only]` | Hardcoded strings & magic numbers |
| `reflex grep <regex>` | Live regex search over contents |
| `reflex ask "<question>"` | Natural-language answer with citations |
| `reflex init` | Create a brain in the current directory |

Commands work from any subdirectory — `reflex` walks up to find the brain. Every list command takes `--json` and `--limit`.

## Configuration (`ask`)

`reflex ask` needs an LLM endpoint. Resolution order: environment variable → `.env.local` → `.env` (at the brain root) → `~/.config/reflex/.env` (one setup for all your repos).

```bash
# ~/.config/reflex/.env   (or export in your shell)
KIMI_API_KEY=sk-...
KIMI_BASE_URL=https://api.kimi.com/coding   # any OpenAI/Anthropic-compatible endpoint
KIMI_API_STYLE=anthropic                     # "openai" (default) or "anthropic"
KIMI_MODEL=kimi-k2-0905-preview              # optional
```

Examples of compatible setups: Kimi (as above), OpenAI (`https://api.openai.com/v1`, style `openai`), or a local model served through an Ollama/llama.cpp OpenAI-compatible proxy.

## Use with AI agents (MCP)

`reflex mcp` starts a [Model Context Protocol](https://modelcontextprotocol.io) server on stdio. Any MCP-capable agent — Kimi Code, Claude Code, Cursor, and others — can then query the brain directly; tools appear to the agent as `mcp__reflex__*`. No local paths needed in the config, just the command:

```json
{
  "mcpServers": {
    "reflex": { "command": "reflex", "args": ["mcp"] }
  }
}
```

- **Kimi Code**: save as `.kimi-code/mcp.json` in the project (per-repo) or `~/.kimi-code/mcp.json` (all projects); manage with `/mcp-config`, check status with `/mcp`
- **Claude Code**: `claude mcp add reflex -- reflex mcp`
- **Cursor**: paste the same `mcpServers` object into Cursor's MCP settings

| Tool | Purpose |
|---|---|
| `brain_stats` | Size up the repo before exploring — call first |
| `find_symbols` | Locate classes, enums, functions, constants by name/kind |
| `list_issues` | Syntax errors, TODO/FIXME, magic numbers, hardcoded strings |
| `outline_file` | Full map of one file before editing it |
| `search_code` | Regex over contents when the index is not enough |
| `refresh_brain` | Re-index after the agent edits code |
| `ask_brain` | Natural-language Q&A with file:line citations |

The server finds the brain by walking up from the agent's working directory, so index the project once first (`reflex init && reflex index`).

## How it works

```
source files ──tree-sitter──▶ symbols + issues ──▶ SQLite (+FTS5)  = the brain
      ▲                                                    │
      └──────── notify watcher ── hash-gated refresh ──────┘
```

1. Each file is parsed by its tree-sitter grammar into a symbol table and an issue list.
2. Results land in `.reflex/brain.db` — plain SQLite with a full-text index over every symbol.
3. Refresh is surgical: a size/mtime pre-filter, then a content hash; unchanged files are never re-read, changed files replace only their own rows.
4. Queries are SQL + FTS5 — that's why answers are instant even on multi-thousand-file repos.

## Roadmap

- MCP server so AI assistants can query the brain directly
- More grammars (Kotlin, Swift, Ruby, PHP, YAML/Toml) — each is a one-table entry in `src/lang.rs`
- Semantic (embedding) search alongside FTS
- TUI dashboard
- Class-based enum detection for Python/Ruby

## Versioning

This project follows [Semantic Versioning](https://semver.org) and keeps a [CHANGELOG](CHANGELOG.md). Released versions are tagged `vX.Y.Z` and published as GitHub Releases.

## License

[MIT + Commons Clause](LICENSE) — free to use, modify, and redistribute,
including commercially; **reselling the software is not permitted**. Note this
makes code-reflex *source-available* (fair-code) rather than OSI-open-source.
