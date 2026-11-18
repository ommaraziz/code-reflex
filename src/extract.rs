use crate::lang::{LangSpec, SymbolKind};
use std::collections::VecDeque;
use std::sync::OnceLock;
use tree_sitter::{Node, TreeCursor};

pub struct SymbolRec {
    pub kind: SymbolKind,
    pub name: String,
    pub signature: Option<String>,
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub parent: Option<String>,
}

pub struct IssueRec {
    pub kind: &'static str,
    pub severity: &'static str,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

pub struct Extracted {
    pub symbols: Vec<SymbolRec>,
    pub issues: Vec<IssueRec>,
}

const MAX_SYMBOLS: usize = 2000;
const MAX_ISSUES: usize = 500;

const COMMON_NUMBERS: &[&str] = &[
    "0", "1", "2", "3", "10", "100", "1000", "-1", "0.5", "0.0", "1.0", "2.0", "100.0",
];

fn todo_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(TODO|FIXME|HACK|XXX|BUG)\b\s*:?[ \t]*(.{0,120})").unwrap()
    })
}

pub fn extract(spec: &LangSpec, code: &str) -> Extracted {
    let mut out = Extracted { symbols: Vec::new(), issues: Vec::new() };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&spec.language).is_err() {
        return out;
    }
    let Some(tree) = parser.parse(code.as_bytes(), None) else { return out };
    let root = tree.root_node();
    walk_symbols(root, spec, code, &mut Vec::new(), &mut Vec::new(), &mut out);
    walk_issues(root, spec, code, &mut out);
    out
}

fn walk_symbols(
    node: Node,
    spec: &LangSpec,
    code: &str,
    ancestors: &mut Vec<&str>,
    parents: &mut Vec<String>,
    out: &mut Extracted,
) {
    if out.symbols.len() >= MAX_SYMBOLS {
        return;
    }
    let kind = node.kind();
    let mut recorded: Option<String> = None;
    if let Some((_, base)) = spec.symbols.iter().find(|(k, _)| *k == kind) {
        let mut sk = *base;
        if spec.name == "go" && kind == "type_spec" {
            sk = go_type_kind(node);
        }
        if sk == SymbolKind::Function && ancestors.iter().any(|a| spec.method_parents.contains(a)) {
            sk = SymbolKind::Method;
        }
        if let Some(name_node) = find_name(node) {
            if let Ok(name) = name_node.utf8_text(code.as_bytes()) {
                let name = name.to_string();
                out.symbols.push(SymbolRec {
                    kind: sk,
                    signature: signature_of(node, code, sk),
                    line: node.start_position().row + 1,
                    col: node.start_position().column + 1,
                    end_line: node.end_position().row + 1,
                    parent: parents.last().cloned(),
                    name,
                });
                if sk == SymbolKind::Enum {
                    let enum_name = out.symbols.last().unwrap().name.clone();
                    collect_variants(node, spec, code, &enum_name, out);
                }
                recorded = Some(out.symbols.last().unwrap().name.clone());
            }
        }
    }
    ancestors.push(kind);
    if let Some(n) = &recorded {
        parents.push(n.clone());
    }
    let mut cursor: TreeCursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_symbols(child, spec, code, ancestors, parents, out);
    }
    if recorded.is_some() {
        parents.pop();
    }
    ancestors.pop();
}

fn go_type_kind(node: Node) -> SymbolKind {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "struct_type" => return SymbolKind::Struct,
            "interface_type" => return SymbolKind::Interface,
            _ => {}
        }
    }
    SymbolKind::TypeAlias
}

fn collect_variants(node: Node, spec: &LangSpec, code: &str, enum_name: &str, out: &mut Extracted) {
    let mut queue: VecDeque<(Node, usize)> = VecDeque::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        queue.push_back((child, 1));
    }
    while let Some((n, depth)) = queue.pop_front() {
        if out.symbols.len() >= MAX_SYMBOLS {
            return;
        }
        if spec.variant_kinds.contains(&n.kind()) {
            if let Some(nn) = find_name(n) {
                if let Ok(text) = nn.utf8_text(code.as_bytes()) {
                    out.symbols.push(SymbolRec {
                        kind: SymbolKind::Variant,
                        name: text.to_string(),
                        signature: None,
                        line: n.start_position().row + 1,
                        col: n.start_position().column + 1,
                        end_line: n.end_position().row + 1,
                        parent: Some(enum_name.to_string()),
                    });
                }
            }
            continue;
        }
        if depth < 3 {
            let mut c = n.walk();
            for child in n.named_children(&mut c) {
                queue.push_back((child, depth + 1));
            }
        }
    }
}

fn find_name(node: Node) -> Option<Node> {
    if let Some(n) = node.child_by_field_name("name") {
        return Some(n);
    }
    let ident_kinds = ["identifier", "type_identifier", "property_identifier", "field_identifier"];
    let mut queue: VecDeque<(Node, usize)> = VecDeque::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        queue.push_back((child, 1));
    }
    let mut visited = 0;
    while let Some((n, depth)) = queue.pop_front() {
        visited += 1;
        if visited > 64 {
            break;
        }
        if depth <= 3 {
            if let Some(nm) = n.child_by_field_name("name") {
                return Some(nm);
            }
            if ident_kinds.contains(&n.kind()) {
                return Some(n);
            }
        }
        if depth < 3 {
            let mut c = n.walk();
            for child in n.named_children(&mut c) {
                queue.push_back((child, depth + 1));
            }
        }
    }
    None
}

fn signature_of(node: Node, code: &str, kind: SymbolKind) -> Option<String> {
    if !matches!(kind, SymbolKind::Function | SymbolKind::Method) {
        return None;
    }
    let bytes = code.as_bytes();
    let span: Option<std::ops::Range<usize>> = if let Some(p) = node.child_by_field_name("parameters") {
        Some(node.start_byte()..p.end_byte())
    } else {
        None
    };
    let text = span
        .filter(|r| r.end <= bytes.len() && r.start < r.end)
        .and_then(|r| std::str::from_utf8(&bytes[r]).ok())
        .or_else(|| node.utf8_text(bytes).ok())?;
    let sig: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_chars(&sig, 120))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn first_line(code: &str, node: Node, max: usize) -> String {
    node.utf8_text(code.as_bytes())
        .map(|t| truncate_chars(t.lines().next().unwrap_or("").trim(), max))
        .unwrap_or_default()
}

fn walk_issues(node: Node, spec: &LangSpec, code: &str, out: &mut Extracted) {
    if out.issues.len() >= MAX_ISSUES {
        return;
    }
    let kind = node.kind();
    let start = node.start_position();
    if kind == "ERROR" {
        out.issues.push(IssueRec {
            kind: "syntax_error",
            severity: "error",
            line: start.row + 1,
            col: start.column + 1,
            message: format!("parse error: {}", first_line(code, node, 80)),
        });
    } else if let Some(missing) = kind.strip_prefix("MISSING") {
        out.issues.push(IssueRec {
            kind: "syntax_error",
            severity: "error",
            line: start.row + 1,
            col: start.column + 1,
            message: format!("missing {}", missing.trim()),
        });
    } else if spec.comment_kinds.contains(&kind) {
        if let Ok(text) = node.utf8_text(code.as_bytes()) {
            if let Some(cap) = todo_re().captures(text) {
                let tag = cap[1].to_ascii_uppercase();
                let (ikind, severity): (&'static str, &'static str) = match tag.as_str() {
                    "TODO" => ("todo", "warn"),
                    "FIXME" => ("fixme", "warn"),
                    "HACK" => ("hack", "info"),
                    "XXX" => ("hack", "info"),
                    _ => ("bug", "warn"),
                };
                let note = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                out.issues.push(IssueRec {
                    kind: ikind,
                    severity,
                    line: start.row + 1,
                    col: start.column + 1,
                    message: if note.is_empty() { tag } else { format!("{}: {}", tag, truncate_chars(note, 100)) },
                });
            }
        }
    } else if spec.detect_hardcoded {
        if spec.string_kinds.contains(&kind) && !is_skipped_string(node, spec) {
            if let Ok(text) = node.utf8_text(code.as_bytes()) {
                let content = text.trim().trim_matches(['"', '\'', '`']);
                if content.chars().count() >= 3 && !content.contains('\n') {
                    out.issues.push(IssueRec {
                        kind: "hardcoded_string",
                        severity: "info",
                        line: start.row + 1,
                        col: start.column + 1,
                        message: format!("\"{}\"", truncate_chars(content, 90)),
                    });
                }
            }
        } else if spec.number_kinds.contains(&kind) && !inside_enum(node) {
            if let Ok(text) = node.utf8_text(code.as_bytes()) {
                let num = text.trim();
                if !COMMON_NUMBERS.contains(&num) {
                    out.issues.push(IssueRec {
                        kind: "magic_number",
                        severity: "info",
                        line: start.row + 1,
                        col: start.column + 1,
                        message: format!("magic number {}", truncate_chars(num, 30)),
                    });
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_issues(child, spec, code, out);
    }
}

fn inside_enum(node: Node) -> bool {
    let mut cur = node.parent();
    let mut steps = 0;
    while let Some(p) = cur {
        if p.kind().contains("enum") {
            return true;
        }
        cur = p.parent();
        steps += 1;
        if steps > 6 {
            break;
        }
    }
    false
}

fn is_skipped_string(node: Node, spec: &LangSpec) -> bool {
    let mut cur = node.parent();
    let mut steps = 0;
    while let Some(p) = cur {
        let k = p.kind();
        if matches!(
            k,
            "import_statement"
                | "import_declaration"
                | "export_statement"
                | "preproc_include"
                | "use_declaration"
                | "ERROR"
        ) {
            return true;
        }
        if spec.name == "python" && k == "expression_statement" {
            if let Some(g) = p.parent() {
                if matches!(g.kind(), "block" | "module") && g.named_child(0) == Some(p) {
                    return true;
                }
            }
        }
        cur = p.parent();
        steps += 1;
        if steps > 6 {
            break;
        }
    }
    false
}
