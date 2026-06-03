//! Tree-sitter backed source extraction for Pi semantic code graphs.
//!
//! This crate owns language-specific parsing. Callers keep policy, storage, and
//! graph-shaping decisions outside the extractor layer.

#![forbid(unsafe_code)]
#![allow(clippy::missing_const_for_fn)]

use std::path::Path;
use tree_sitter::{Language, Node as TreeSitterNode, Parser as TreeSitterParser};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeGraph {
    pub language_id: String,
    pub symbols: Vec<ExtractedCodeSymbol>,
    pub calls: Vec<ExtractedCodeCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeSymbol {
    pub kind: String,
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCodeCall {
    pub caller: String,
    pub callee: String,
    pub line: usize,
}

pub trait CodeLanguageExtractor: Sync {
    fn language_id(&self) -> &'static str;
    fn supports_path(&self, source_path: &str) -> bool;
    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph>;
}

static RUST_TREE_SITTER_EXTRACTOR: RustTreeSitterExtractor = RustTreeSitterExtractor;
static GO_TREE_SITTER_EXTRACTOR: GoTreeSitterExtractor = GoTreeSitterExtractor;

static EXTRACTORS: [&dyn CodeLanguageExtractor; 2] =
    [&RUST_TREE_SITTER_EXTRACTOR, &GO_TREE_SITTER_EXTRACTOR];

#[must_use]
pub fn extractor_for_path(source_path: &str) -> Option<&'static dyn CodeLanguageExtractor> {
    EXTRACTORS
        .iter()
        .copied()
        .find(|extractor| extractor.supports_path(source_path))
}

#[must_use]
pub fn extract_code_graph(source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
    extractor_for_path(source_path)?.extract(source_path, content)
}

#[derive(Debug, Clone, Copy)]
pub struct RustTreeSitterExtractor;

impl CodeLanguageExtractor for RustTreeSitterExtractor {
    fn language_id(&self) -> &'static str {
        "rust"
    }

    fn supports_path(&self, source_path: &str) -> bool {
        has_extension(source_path, "rs")
    }

    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
        if !self.supports_path(source_path) {
            return None;
        }
        parse_tree_sitter_ast(
            self.language_id(),
            &tree_sitter_rust::LANGUAGE.into(),
            content,
            collect_rust_ast_symbols,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GoTreeSitterExtractor;

impl CodeLanguageExtractor for GoTreeSitterExtractor {
    fn language_id(&self) -> &'static str {
        "go"
    }

    fn supports_path(&self, source_path: &str) -> bool {
        has_extension(source_path, "go")
    }

    fn extract(&self, source_path: &str, content: &str) -> Option<ExtractedCodeGraph> {
        if !self.supports_path(source_path) {
            return None;
        }
        parse_tree_sitter_ast(
            self.language_id(),
            &tree_sitter_go::LANGUAGE.into(),
            content,
            collect_go_ast_symbols,
        )
    }
}

type AstCollector = fn(
    TreeSitterNode<'_>,
    &[u8],
    &mut Vec<ExtractedCodeSymbol>,
    &mut Vec<ExtractedCodeCall>,
    Option<&str>,
    bool,
);

fn parse_tree_sitter_ast(
    language_id: &str,
    language: &Language,
    content: &str,
    collect: AstCollector,
) -> Option<ExtractedCodeGraph> {
    let mut parser = TreeSitterParser::new();
    parser.set_language(language).ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }

    let bytes = content.as_bytes();
    let mut symbols = Vec::new();
    let mut calls = Vec::new();
    collect(root, bytes, &mut symbols, &mut calls, None, false);
    Some(normalize_extraction(language_id, symbols, calls))
}

fn normalize_extraction(
    language_id: &str,
    mut symbols: Vec<ExtractedCodeSymbol>,
    mut calls: Vec<ExtractedCodeCall>,
) -> ExtractedCodeGraph {
    symbols.sort_by(|left, right| {
        left.line_start
            .cmp(&right.line_start)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });
    symbols.dedup_by(|left, right| {
        left.kind == right.kind && left.name == right.name && left.line_start == right.line_start
    });
    calls.sort_by(|left, right| {
        left.caller
            .cmp(&right.caller)
            .then_with(|| left.callee.cmp(&right.callee))
            .then_with(|| left.line.cmp(&right.line))
    });
    calls.dedup();
    ExtractedCodeGraph {
        language_id: language_id.to_string(),
        symbols,
        calls,
    }
}

fn collect_rust_ast_symbols(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    symbols: &mut Vec<ExtractedCodeSymbol>,
    calls: &mut Vec<ExtractedCodeCall>,
    current_symbol: Option<&str>,
    pending_test_attribute: bool,
) {
    let mut cursor = node.walk();
    let mut next_test_attribute = pending_test_attribute;
    for child in node.named_children(&mut cursor) {
        if is_rust_test_attribute_node(child, bytes) {
            next_test_attribute = true;
            continue;
        }

        if let Some(symbol) = rust_symbol_from_node(child, bytes, next_test_attribute) {
            let symbol_name = symbol.name.clone();
            let scan_calls = matches!(symbol.kind.as_str(), "fn" | "trait_fn");
            symbols.push(symbol);
            if scan_calls {
                collect_rust_ast_symbols(child, bytes, symbols, calls, Some(&symbol_name), false);
            } else {
                collect_rust_ast_symbols(child, bytes, symbols, calls, None, false);
            }
            next_test_attribute = false;
            continue;
        }

        if let Some(caller) = current_symbol
            && let Some(callee) = rust_call_name_from_node(child, bytes)
        {
            calls.push(ExtractedCodeCall {
                caller: caller.to_string(),
                callee,
                line: one_indexed_row(child),
            });
        }

        collect_rust_ast_symbols(child, bytes, symbols, calls, current_symbol, false);
        next_test_attribute = false;
    }
}

fn rust_symbol_from_node(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    is_test: bool,
) -> Option<ExtractedCodeSymbol> {
    let kind = match node.kind() {
        "function_item" => "fn",
        "function_signature_item" => "trait_fn",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        "impl_item" => "impl",
        "mod_item" => "mod",
        "type_item" => "type",
        "const_item" => "const",
        "static_item" => "static",
        _ => return None,
    };
    let name = if node.kind() == "impl_item" {
        rust_impl_name(node, bytes)?
    } else {
        node.child_by_field_name("name")
            .and_then(|name| node_text(name, bytes))
            .map(ToString::to_string)?
    };
    Some(ExtractedCodeSymbol {
        kind: kind.to_string(),
        name,
        line_start: one_indexed_row(node),
        line_end: node.end_position().row.saturating_add(1),
        is_test: is_test || rust_node_has_test_attribute(node, bytes),
    })
}

fn rust_node_has_test_attribute(node: TreeSitterNode<'_>, bytes: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| is_rust_test_attribute_node(child, bytes))
}

fn rust_impl_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("type")
        .and_then(|node| node_text(node, bytes))
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .filter_map(|child| node_text(child, bytes))
                .find(|text| !matches!(*text, "impl" | "for"))
        })
        .map(|text| format!("impl {}", collapse_ws(text)))
}

fn rust_call_name_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            rust_callable_name(function, bytes)
        }
        "macro_invocation" => node
            .child_by_field_name("macro")
            .and_then(|macro_node| node_text(macro_node, bytes))
            .map(|name| format!("{name}!")),
        _ => None,
    }
}

fn rust_callable_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, bytes).map(ToString::to_string),
        "scoped_identifier" => node_text(node, bytes)
            .and_then(|text| text.rsplit("::").next())
            .filter(|name| !name.is_empty())
            .map(ToString::to_string),
        "generic_function" => node
            .child_by_field_name("function")
            .and_then(|function| rust_callable_name(function, bytes)),
        "field_expression" => node
            .child_by_field_name("field")
            .and_then(|field| node_text(field, bytes))
            .map(ToString::to_string),
        _ => node_text(node, bytes).map(collapse_ws),
    }
}

fn is_rust_test_attribute_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> bool {
    if !matches!(node.kind(), "attribute_item" | "inner_attribute_item") {
        return false;
    }
    node_text(node, bytes).is_some_and(|text| {
        let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        compact == "#[test]"
            || compact.starts_with("#[tokio::test")
            || compact.starts_with("#[asupersync::test")
            || compact.starts_with("#[should_panic")
    })
}

fn collect_go_ast_symbols(
    node: TreeSitterNode<'_>,
    bytes: &[u8],
    symbols: &mut Vec<ExtractedCodeSymbol>,
    calls: &mut Vec<ExtractedCodeCall>,
    current_symbol: Option<&str>,
    _pending_test_attribute: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(symbol) = go_symbol_from_node(child, bytes) {
            let symbol_name = symbol.name.clone();
            let scan_calls = matches!(symbol.kind.as_str(), "func" | "method");
            symbols.push(symbol);
            if scan_calls {
                collect_go_ast_symbols(child, bytes, symbols, calls, Some(&symbol_name), false);
            } else {
                collect_go_ast_symbols(child, bytes, symbols, calls, None, false);
            }
            continue;
        }

        if let Some(caller) = current_symbol
            && let Some(callee) = go_call_name_from_node(child, bytes)
        {
            calls.push(ExtractedCodeCall {
                caller: caller.to_string(),
                callee,
                line: one_indexed_row(child),
            });
        }

        collect_go_ast_symbols(child, bytes, symbols, calls, current_symbol, false);
    }
}

fn go_symbol_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<ExtractedCodeSymbol> {
    let (kind, name) = match node.kind() {
        "function_declaration" => (
            "func",
            node.child_by_field_name("name")
                .and_then(|name| node_text(name, bytes))
                .map(ToString::to_string)?,
        ),
        "method_declaration" => ("method", go_method_name(node, bytes)?),
        "type_declaration" => ("type", go_type_declaration_name(node, bytes)?),
        _ => return None,
    };
    Some(ExtractedCodeSymbol {
        kind: kind.to_string(),
        is_test: is_go_test_symbol(kind, &name),
        name,
        line_start: one_indexed_row(node),
        line_end: node.end_position().row.saturating_add(1),
    })
}

fn go_method_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    let name = node
        .child_by_field_name("name")
        .and_then(|name| node_text(name, bytes))?;
    let receiver = node
        .child_by_field_name("receiver")
        .and_then(|receiver| node_text(receiver, bytes))
        .map(go_receiver_type_name);
    receiver.map_or_else(
        || Some(name.to_string()),
        |receiver| Some(format!("{receiver}.{name}")),
    )
}

fn go_receiver_type_name(receiver: &str) -> String {
    let trimmed = receiver
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let tokens: Vec<&str> = trimmed
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '*' || ch == '[' || ch == ']')
        })
        .filter(|token| !token.is_empty())
        .collect();
    tokens.last().map_or_else(
        || collapse_ws(trimmed),
        |token| token.trim_start_matches('*').to_string(),
    )
}

fn go_type_declaration_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| match child.kind() {
            "type_spec" => child
                .child_by_field_name("name")
                .and_then(|name| node_text(name, bytes))
                .map(ToString::to_string),
            _ => None,
        })
}

fn is_go_test_symbol(kind: &str, name: &str) -> bool {
    kind == "func"
        && (name.starts_with("Test")
            || name.starts_with("Benchmark")
            || name.starts_with("Example"))
}

fn go_call_name_from_node(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    go_callable_name(function, bytes)
}

fn go_callable_name(node: TreeSitterNode<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, bytes).map(ToString::to_string),
        "selector_expression" => node
            .child_by_field_name("field")
            .and_then(|field| node_text(field, bytes))
            .map(ToString::to_string)
            .or_else(|| {
                node_text(node, bytes)
                    .and_then(|text| text.rsplit('.').next())
                    .filter(|name| !name.is_empty())
                    .map(ToString::to_string)
            }),
        _ => node_text(node, bytes).map(collapse_ws),
    }
}

fn node_text<'a>(node: TreeSitterNode<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(bytes).ok()
}

fn one_indexed_row(node: TreeSitterNode<'_>) -> usize {
    node.start_position().row.saturating_add(1)
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_extension(source_path: &str, extension: &str) -> bool {
    Path::new(source_path)
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
mod tests {
    use super::{ExtractedCodeCall, extract_code_graph};

    #[test]
    fn rust_extractor_indexes_symbols_and_calls() {
        let graph = extract_code_graph(
            "src/lib.rs",
            r"
                struct Agent;

                #[test]
                fn smoke() {
                    helper();
                    value.render();
                }
            ",
        )
        .expect("rust graph");

        assert_eq!(graph.language_id, "rust");
        assert!(graph.symbols.iter().any(|symbol| {
            symbol.kind == "struct" && symbol.name == "Agent" && !symbol.is_test
        }));
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| { symbol.kind == "fn" && symbol.name == "smoke" && symbol.is_test })
        );
        assert_has_call(&graph.calls, "smoke", "helper");
        assert_has_call(&graph.calls, "smoke", "render");
    }

    #[test]
    fn go_extractor_indexes_symbols_and_calls() {
        let graph = extract_code_graph(
            "src/server.go",
            r"
                package server

                type Agent struct{}

                func helper() {}

                func TestSmoke(t *testing.T) {
                    helper()
                    value.Render()
                }

                func (a *Agent) Run() {
                    helper()
                }
            ",
        )
        .expect("go graph");

        assert_eq!(graph.language_id, "go");
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| symbol.kind == "type" && symbol.name == "Agent")
        );
        assert!(graph.symbols.iter().any(|symbol| {
            symbol.kind == "func" && symbol.name == "TestSmoke" && symbol.is_test
        }));
        assert!(
            graph
                .symbols
                .iter()
                .any(|symbol| symbol.kind == "method" && symbol.name == "Agent.Run")
        );
        assert_has_call(&graph.calls, "TestSmoke", "helper");
        assert_has_call(&graph.calls, "TestSmoke", "Render");
        assert_has_call(&graph.calls, "Agent.Run", "helper");
    }

    fn assert_has_call(calls: &[ExtractedCodeCall], caller_name: &str, callee_name: &str) {
        assert!(
            calls
                .iter()
                .any(|call| call.caller == caller_name && call.callee == callee_name),
            "missing {caller_name} -> {callee_name}"
        );
    }
}
