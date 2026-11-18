# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org).

## [0.2.0] - 2026-11-18

### Added

- `reflex mcp`: an MCP (Model Context Protocol) server on stdio — hand-rolled JSON-RPC, no new dependencies — exposing the brain to any MCP-capable AI agent (Kimi Code, Claude Code, Cursor, ...). Tools: `brain_stats`, `find_symbols`, `list_issues`, `outline_file`, `search_code`, `refresh_brain`, `ask_brain`. The server locates the brain by walking up from the agent's working directory.
- README: "Use with AI agents (MCP)" setup section.

[0.2.0]: https://github.com/ommaraziz/code-reflex/releases/tag/v0.2.0


## [0.1.0] - 2026-11-18

### Added

- Licensed under MIT + Commons Clause (free to use/modify/redistribute; resale prohibited)

- Initial release of `code-reflex` (CLI: `reflex`)
- Per-repo brain in SQLite (`.reflex/brain.db`) with FTS5 full-text search over all symbols
- Symbol extraction via tree-sitter for Rust, Python, JavaScript/JSX, TypeScript/TSX, Go, Java, C, C++, C#, Bash, and JSON: classes, structs, enums + variants, traits/interfaces, functions/methods, constants, fields — with signatures and parent names
- Issue detection: syntax errors (parse-tree `ERROR`/`MISSING` nodes), TODO/FIXME/HACK/XXX/BUG comments, magic numbers, hardcoded strings
- Incremental indexing gated by content hash (size/mtime pre-filter); `reflex index --force` for full rebuilds; pruning of deleted files
- `reflex watch` daemon: debounced, per-file surgical refresh on save
- Query commands: `symbols`, `classes`, `structs`, `enums`, `fns`, `traits`, `consts`, `outline`, `issues`, `errors`, `hardcoded`, `grep`, `stats` — with `--json` and `--limit` on list commands
- `reflex ask`: natural-language Q&A over the indexed code with `file:line` citations; supports OpenAI-compatible and Anthropic-compatible endpoints (config via `KIMI_API_KEY`/`KIMI_BASE_URL`/`KIMI_MODEL`/`KIMI_API_STYLE`, resolved from env → repo `.env.local`/`.env` → `~/.config/reflex/.env`)
- SIGPIPE-safe output for Unix pipelines

[0.1.0]: https://github.com/ommaraziz/code-reflex/releases/tag/v0.1.0
