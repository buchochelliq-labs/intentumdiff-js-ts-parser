//! JavaScript / TypeScript parser plugin - full-parse mode.
//!
//! Handles `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.mts`, `.cts`, `.tsx` files.
//! Parses source with tree-sitter-javascript or tree-sitter-typescript directly.
//! This plugin walks the CST and emits a
//! SemanticNode tree focused on semantically meaningful constructs.
//!
//! Grammar IDs returned by `detect_language`:
//!   javascript — .js / .mjs / .cjs / .jsx
//!   typescript — .ts / .mts / .cts
//!   tsx        — .tsx

use intentumdiff_plugin_sdk::{
    cst::CstNode,
    hash::structural_hash_with_memo,
    tree::{NodeFacts, SemanticNode, SemanticNodeBuilder},
};

wit_bindgen::generate!({
    path: "wit/plugin.wit",
    world: "parser-plugin",
});

use crate::exports::intentdiff::plugin::parser::ExamplePair;
use crate::exports::intentdiff::plugin::parser::Guest;
use crate::exports::intentdiff::plugin::parser::LanguageInfoRecord;
use crate::exports::intentdiff::plugin::parser::ParserMode;

const PLUGIN_METADATA: &str = include_str!("../plugin_metadata.info");

fn language_info_for(ids: Vec<String>) -> Vec<LanguageInfoRecord> {
    let metadata = intentumdiff_plugin_sdk::metadata::parse_plugin_metadata(PLUGIN_METADATA);
    ids.into_iter()
        .map(|language_id| {
            let info = metadata.language_or_default(&language_id);
            LanguageInfoRecord {
                language_id: info.language_id,
                language_name: info.language_name,
                language_short_name: info.language_short_name,
                monaco_language: info.monaco_language,
                default_filename: info.default_filename,
                language_file_extensions: info.language_file_extensions,
                author: metadata.author().to_string(),
                plugin_version: metadata.plugin_version().to_string(),
                last_updated: metadata.last_updated().to_string(),
            }
        })
        .collect()
}
struct JsTsParser;

// ---------------------------------------------------------------------------
// Trivia node types to strip
// ---------------------------------------------------------------------------
const TRIVIA: &[&str] = &[
    "comment",
    "line_comment",
    "block_comment",
    "html_comment",
    "whitespace",
    "automatic_semicolon",
];

// ---------------------------------------------------------------------------
// Semantic node types to preserve (JavaScript + TypeScript)
// ---------------------------------------------------------------------------
const SEMANTIC_TYPES: &[&str] = &[
    // Root
    "program",
    // Functions
    "function_declaration",
    "function",
    "arrow_function",
    "generator_function_declaration",
    "generator_function",
    // `async` variants (see `async_variant_of`). tree-sitter encodes `async` as an
    // ANONYMOUS keyword token, so a distinct node type is the only way async-ness
    // survives into the tree. Each variant is listed here exactly when its non-async
    // counterpart above is, so async code keeps its counterpart's shape.
    "async_function_declaration",
    "async_function",
    "async_arrow_function",
    "async_generator_function_declaration",
    "async_generator_function",
    "formal_parameters",
    "required_parameter",
    "optional_parameter",
    // Classes
    "class_declaration",
    "class",
    "abstract_class_declaration",
    "method_definition",
    "async_method_definition",
    "field_definition",
    // Variables
    "variable_declaration",
    "lexical_declaration",
    "variable_declarator",
    // Imports / exports
    "import_statement",
    "export_statement",
    "export_default_declaration",
    // Control flow
    "expression_statement",
    "return_statement",
    "if_statement",
    "else_clause",
    "for_statement",
    "for_in_statement",
    "for_of_statement",
    "while_statement",
    "do_statement",
    "try_statement",
    "catch_clause",
    "finally_clause",
    "throw_statement",
    "switch_statement",
    "switch_case",
    "switch_default",
    "break_statement",
    "continue_statement",
    "labeled_statement",
    "with_statement",
    "debugger_statement",
    // Expressions
    "assignment_expression",
    "augmented_assignment_expression",
    "binary_expression",
    "call_expression",
    "member_expression",
    "parenthesized_expression",
    "string",
    "template_string",
    "unary_expression",
    "new_expression",
    "await_expression",
    "yield_expression",
    "ternary_expression",
    // TypeScript declarations
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
    "abstract_method_signature",
    "method_signature",
    "property_signature",
    "index_signature",
    "construct_signature",
    "call_signature",
    "module",
    "namespace_export",
    "import_alias",
    "ambient_declaration",
    // Identifiers and literals stay in the CST for labels/hashes, but are not
    // emitted as standalone semantic nodes. Keeping them as leaf changes made
    // tiny TypeScript files produce oversized semantic graphs and burn fuel
    // without adding useful review intent.
    // Destructuring / patterns
    "object_pattern",
    "array_pattern",
    "rest_pattern",
    "object",
    "array",
    "spread_element",
    "pair",
    "shorthand_property_identifier",
    // TypeScript type nodes
    "type_annotation",
    "predefined_type",
    "literal_type",
    "union_type",
    "intersection_type",
    "array_type",
    "tuple_type",
    "object_type",
    "function_type",
    "constructor_type",
    "type_query",
    "index_type_query",
    "lookup_type",
    "conditional_type",
    "infer_type",
    "mapped_type_clause",
    "template_literal_type",
    "generic_type",
    "type_parameters",
    "type_arguments",
    "as_expression",
    "satisfies_expression",
    "non_null_expression",
    "type_assertion",
    "readonly_type",
    // Decorators
    "decorator",
    // Modern syntax (ES2022 + TS 5.x)
    "using_declaration",
    "await_using_declaration",
    "class_static_block",
    // JSX
    "jsx_element",
    "jsx_self_closing_element",
    "jsx_opening_element",
    "jsx_closing_element",
    "jsx_attribute",
    "jsx_expression",
    // Import / export granularity
    "import_specifier",
    "namespace_import",
    "named_imports",
    "export_specifier",
];

fn is_semantic(node_type: &str) -> bool {
    SEMANTIC_TYPES.contains(&node_type)
}

fn is_signature_name_leaf(node_type: &str) -> bool {
    matches!(
        node_type,
        "identifier"
            | "property_identifier"
            | "private_property_identifier"
            | "shorthand_property_identifier"
            | "type_identifier"
            | "statement_identifier"
    )
}

fn is_literal_evidence_leaf(node_type: &str) -> bool {
    matches!(
        node_type,
        "string" | "string_fragment" | "template_string" | "number" | "regex"
    )
}

fn is_signature_context(node_type: &str) -> bool {
    matches!(
        node_type,
        "formal_parameters"
            | "required_parameter"
            | "optional_parameter"
            | "rest_pattern"
            | "pair_pattern"
            | "object_pattern"
            | "array_pattern"
            | "type_annotation"
            | "type_parameters"
            | "type_arguments"
    )
}

fn is_review_name_context(node_type: &str) -> bool {
    is_signature_context(node_type)
        || matches!(
            node_type,
            "member_expression"
                | "call_expression"
                | "binary_expression"
                | "object"
                | "pair"
                | "parenthesized_expression"
                | "unary_expression"
                | "for_in_statement"
                | "for_of_statement"
                | "lexical_declaration"
                | "variable_declaration"
                | "variable_declarator"
        )
}

/// Extract a human-readable label from a CST node.
fn leaf_text(node: &CstNode) -> String {
    if node.is_leaf() {
        return node.text_or_empty().to_string();
    }
    node.children
        .iter()
        .map(leaf_text)
        .collect::<Vec<String>>()
        .join("")
}

fn label_for(node: &CstNode) -> String {
    if node.is_leaf() {
        return node.text_or_empty().to_string();
    }

    match node.node_type.as_str() {
        "string" | "template_string" => {
            let text = leaf_text(node);
            if !text.is_empty() {
                return text;
            }
        }
        // Named declarations: first identifier child is the name
        "function_declaration"
        | "async_function_declaration"
        | "generator_function_declaration"
        | "async_generator_function_declaration"
        | "class_declaration"
        | "abstract_class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "module" => {
            for child in &node.children {
                if child.node_type == "identifier" || child.node_type == "type_identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Methods and field definitions
        "method_definition"
        | "async_method_definition"
        | "field_definition"
        | "method_signature"
        | "property_signature"
        | "abstract_method_signature" => {
            for child in &node.children {
                if matches!(
                    child.node_type.as_str(),
                    "property_identifier"
                        | "identifier"
                        | "string"
                        | "computed_property_name"
                        | "private_property_identifier"
                ) {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Variable declarator: first identifier is the variable name
        "variable_declarator" => {
            for child in &node.children {
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        "member_expression" => {
            let parts: Vec<&str> = node
                .children
                .iter()
                .filter_map(|child| {
                    if is_signature_name_leaf(&child.node_type) {
                        let text = child.text_or_empty();
                        if text.is_empty() {
                            None
                        } else {
                            Some(text)
                        }
                    } else {
                        None
                    }
                })
                .collect();
            if !parts.is_empty() {
                return parts.join(".");
            }
        }
        "formal_parameters" => {
            let names: Vec<String> = node
                .children
                .iter()
                .filter_map(|child| {
                    let label = label_for(child);
                    if label.is_empty() || label == child.node_type {
                        None
                    } else {
                        Some(label)
                    }
                })
                .collect();
            if !names.is_empty() {
                return names.join(", ");
            }
        }
        "required_parameter" | "optional_parameter" | "rest_pattern" => {
            for child in &node.children {
                if is_signature_name_leaf(&child.node_type) {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Import statement: use the module path string as label
        "import_statement" => {
            for child in &node.children {
                if child.node_type == "string" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        "export_statement" | "export_default_declaration" => {
            for child in &node.children {
                if matches!(
                    child.node_type.as_str(),
                    "function_declaration"
                        | "async_function_declaration"
                        | "generator_function_declaration"
                        | "async_generator_function_declaration"
                        | "class_declaration"
                        | "abstract_class_declaration"
                        | "interface_declaration"
                        | "type_alias_declaration"
                        | "enum_declaration"
                        | "variable_declaration"
                        | "lexical_declaration"
                        | "variable_declarator"
                ) {
                    return label_for(child);
                }
            }
        }
        // Decorators: extract the decorator name (e.g. @Injectable → "Injectable")
        "decorator" => {
            for child in &node.children {
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
                // @decorator(args) — call_expression whose function is the name
                if child.node_type == "call_expression" {
                    for inner in &child.children {
                        if inner.node_type == "identifier" || inner.node_type == "member_expression"
                        {
                            return inner.text_or_empty().to_string();
                        }
                    }
                }
            }
        }
        // JSX: extract the tag/component name
        "jsx_opening_element" | "jsx_self_closing_element" => {
            for child in &node.children {
                if matches!(
                    child.node_type.as_str(),
                    "identifier" | "member_expression" | "nested_identifier"
                ) {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // import { foo as bar } — label is the imported name
        "import_specifier" | "export_specifier" => {
            for child in &node.children {
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // import * as ns — label is the alias identifier
        "namespace_import" => {
            for child in &node.children {
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // using x = expr  /  await using x = expr (TS 5.2+)
        "using_declaration" | "await_using_declaration" => {
            for child in &node.children {
                if child.node_type == "variable_declarator" {
                    for inner in &child.children {
                        if inner.node_type == "identifier" {
                            return inner.text_or_empty().to_string();
                        }
                    }
                }
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        _ => {}
    }

    // Generic fallback: first identifier child
    for child in &node.children {
        if child.node_type == "identifier" {
            return child.text_or_empty().to_string();
        }
    }
    node.node_type.clone()
}

fn is_class_like(node_type: &str) -> bool {
    matches!(
        node_type,
        "class_declaration" | "class" | "abstract_class_declaration" | "interface_declaration"
    )
}

fn is_method_like(node_type: &str) -> bool {
    matches!(
        node_type,
        "function_declaration"
            | "function"
            | "arrow_function"
            | "generator_function_declaration"
            | "generator_function"
            | "method_definition"
            | "method_signature"
            | "abstract_method_signature"
    ) || is_async_node_type(node_type)
}

/// The node types minted by [`async_variant_of`]. Kept in one place so every vocabulary
/// that asks "is this a function?" answers the same for `async f` as it does for `f`.
fn is_async_node_type(node_type: &str) -> bool {
    matches!(
        node_type,
        "async_function_declaration"
            | "async_function"
            | "async_function_expression"
            | "async_arrow_function"
            | "async_generator_function_declaration"
            | "async_generator_function"
            | "async_method_definition"
    )
}

fn convert(
    node: &CstNode,
    id_prefix: &str,
    parent_class: Option<&str>,
    parent_semantic_type: Option<&str>,
    memo: &mut std::collections::HashMap<usize, String>,
) -> Option<SemanticNode> {
    // When inside a class-like node, its direct children see the class as
    // their enclosing context for PULL_UP / PUSH_DOWN detection.
    let own_class_label: Option<String> = if is_class_like(&node.node_type) {
        Some(label_for(node))
    } else {
        None
    };
    let child_parent_class: Option<&str> = own_class_label.as_deref().or(parent_class);
    let semantic_here = is_semantic(&node.node_type);
    let child_semantic_type = if semantic_here {
        Some(node.node_type.as_str())
    } else {
        parent_semantic_type
    };

    let children: Vec<SemanticNode> = node
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            convert(
                c,
                &format!("{}.{}", id_prefix, i),
                child_parent_class,
                child_semantic_type,
                memo,
            )
        })
        .collect();
    let signature_leaf =
        node.is_leaf() && is_signature_name_leaf(&node.node_type) && parent_semantic_type
            .map(is_review_name_context)
            .unwrap_or(false);
    let literal_evidence_leaf = node.is_leaf()
        && is_literal_evidence_leaf(&node.node_type)
        && parent_semantic_type
            .map(|node_type| !is_signature_context(node_type))
            .unwrap_or(false);
    if !semantic_here && !signature_leaf && !literal_evidence_leaf && children.is_empty() {
        return None;
    }

    let hash = structural_hash_with_memo(node, memo);

    let mut builder = SemanticNodeBuilder::new(
        id_prefix,
        &node.node_type,
        label_for(node),
        node.start_line,
        node.start_col,
        node.end_line,
        node.end_col,
        hash,
    )
    .children(children);

    // Tag method-like nodes with the enclosing class for PULL_UP / PUSH_DOWN.
    if is_method_like(&node.node_type) {
        if let Some(class_name) = parent_class {
            builder = builder.parent_type(class_name);
        }
    }

    // Privacy-safe facts: async-ness is the one fact this parser can positively determine
    // from the node type alone (a flag, never text/names/literals). The engine derives
    // param_count/returns/body for every Wasm-parsed language from the pruned tree.
    if is_async_node_type(&node.node_type) {
        builder = builder.facts(NodeFacts {
            is_async: Some(true),
            ..NodeFacts::default()
        });
    }

    Some(builder.build())
}

/// The distinct node type for an `async` function/method, or `None` when the node is not a
/// function form or carries no `async` keyword.
///
/// tree-sitter-javascript / -typescript encode `async` as an ANONYMOUS keyword token:
/// `async function f() {}` is a plain `function_declaration` whose `async` child is unnamed.
/// `node_to_cst` walks named children only, so the token was dropped and `function f()` vs
/// `async function f()` serialised byte-identically — the toggle read as STYLE-ONLY even
/// though every call site now receives a Promise (the #30 class). Mirror the python parser's
/// `async_function_def` treatment and mint a distinct type so async-ness survives.
///
/// Unlike python's `async def`, the `async` token is not always first (`static async m()`),
/// so scan the direct children instead of testing `child(0)`. `async` used as a plain name
/// (`function async() {}`) parses as an `identifier`, never as an `async` keyword node, so
/// this cannot false-positive.
fn async_variant_of(node: tree_sitter::Node<'_>) -> Option<&'static str> {
    let variant = match node.kind() {
        "function_declaration" => "async_function_declaration",
        "function" => "async_function",
        "function_expression" => "async_function_expression",
        "arrow_function" => "async_arrow_function",
        "generator_function_declaration" => "async_generator_function_declaration",
        "generator_function" => "async_generator_function",
        "method_definition" => "async_method_definition",
        _ => return None,
    };
    let mut cursor = node.walk();
    let has_async = node.children(&mut cursor).any(|c| c.kind() == "async");
    has_async.then_some(variant)
}

fn node_to_cst(node: tree_sitter::Node<'_>, source: &[u8]) -> CstNode {
    let children: Vec<CstNode> = (0..node.named_child_count())
        .filter_map(|i| node.named_child(i))
        .map(|child| node_to_cst(child, source))
        .collect();

    let text = if children.is_empty() {
        Some(
            node.utf8_text(source)
                .unwrap_or("")
                .chars()
                .take(4096)
                .collect(),
        )
    } else {
        None
    };

    CstNode {
        node_type: async_variant_of(node)
            .map(str::to_string)
            .unwrap_or_else(|| node.kind().to_string()),
        named: node.is_named(),
        text,
        start_line: node.start_position().row as u32,
        start_col: node.start_position().column as u32,
        end_line: node.end_position().row as u32,
        end_col: node.end_position().column as u32,
        children,
    }
}

fn select_language(language: &str, filename: &str) -> String {
    match language {
        "javascript" | "typescript" | "tsx" => language.to_string(),
        _ => JsTsParser::detect_language(filename.to_string(), String::new()),
    }
}

fn parse_source(source: &str, language: &str, filename: &str) -> Result<CstNode, String> {
    let mut parser = tree_sitter::Parser::new();
    let selected = select_language(language, filename);
    let lang = match selected.as_str() {
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => tree_sitter_javascript::LANGUAGE.into(),
    };
    parser
        .set_language(&lang)
        .map_err(|_| format!("Failed to load {} grammar", selected))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Parse failed".to_string())?;
    Ok(node_to_cst(tree.root_node(), source.as_bytes()))
}

fn process_impl(source: &str, language: &str, filename: &str) -> String {
    let root: CstNode = match parse_source(source, language, filename) {
        Ok(n) => n,
        Err(e) => return format!(r#"{{"error":"{}"}}"#, e),
    };
    let mut memo: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let sem = match convert(&root, "0", None, None, &mut memo) {
        Some(n) => n,
        None => return r#"{"error":"Empty semantic tree"}"#.to_string(),
    };
    match serde_json::to_string(&sem) {
        Ok(s) => s,
        Err(e) => format!(r#"{{"error":"Serialisation error: {}"}}"#, e),
    }
}

impl Guest for JsTsParser {
    fn get_parser_mode() -> ParserMode {
        ParserMode::FullParse
    }

    fn grammar_id() -> String {
        "javascript".to_string()
    }

    fn detect_language(filename: String, _content: String) -> String {
        let lower = filename.to_lowercase();
        if lower.ends_with(".tsx") {
            "tsx".to_string()
        } else if lower.ends_with(".ts") || lower.ends_with(".mts") || lower.ends_with(".cts") {
            "typescript".to_string()
        } else if lower.ends_with(".js")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs")
            || lower.ends_with(".jsx")
        {
            "javascript".to_string()
        } else {
            String::new()
        }
    }

    fn preprocess_source(source: String) -> String {
        source
    }

    fn process(input: String, language: String, filename: String) -> String {
        process_impl(&input, &language, &filename)
    }

    fn trivia_node_types() -> Vec<String> {
        TRIVIA.iter().map(|s| s.to_string()).collect()
    }

    fn language_ids() -> Vec<String> {
        vec![
            "javascript".to_string(),
            "typescript".to_string(),
            "tsx".to_string(),
        ]
    }

    fn language_info() -> Vec<LanguageInfoRecord> {
        language_info_for(Self::language_ids())
    }
    fn priority() -> i32 {
        0
    }

    fn example(language: String) -> ExamplePair {
        if language == "typescript" || language == "tsx" {
            return ExamplePair {
                old: "function getUser(id) {\n  return { id: id, name: \"Alice\" };\n}\n\nfunction formatName(user) {\n  return user.name.toUpperCase();\n}\n".to_string(),
                new: "interface User {\n  id: number;\n  name: string;\n}\n\nfunction getUser(id: number): User {\n  return { id, name: \"Alice\" };\n}\n\nfunction formatName(user: User): string {\n  return user.name.toUpperCase();\n}\n".to_string(),
            };
        }
        ExamplePair {
            old: "var PI = 3.14159;\n\nfunction circleArea(radius) {\n  return PI * radius * radius;\n}\n\nfunction greet(name) {\n  return \"Hello, \" + name + \"!\";\n}\n\nmodule.exports = { circleArea, greet };\n".to_string(),
            new: "const PI = 3.14159;\n\nconst circleArea = (radius) => PI * radius ** 2;\n\nconst greet = (name) => `Hello, ${name}!`;\n\nmodule.exports = { circleArea, greet };\n".to_string(),
        }
    }
}

export!(JsTsParser);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::intentdiff::plugin::parser::Guest;
    use intentumdiff_plugin_sdk::testing as t;

    #[test]
    fn grammar_id_nonempty() {
        assert!(!JsTsParser::grammar_id().is_empty());
    }

    #[test]
    fn language_ids_contain_grammar_id() {
        let gid = JsTsParser::grammar_id();
        let ids = JsTsParser::language_ids();
        assert!(
            ids.contains(&gid),
            "language_ids {:?} must contain {:?}",
            ids,
            gid
        );
    }

    #[test]
    fn detect_language_known_ext() {
        let r = JsTsParser::detect_language("test.js".to_string(), "".to_string());
        assert_eq!(r.as_str(), "javascript");
    }

    #[test]
    fn detect_language_unknown_ext() {
        let r =
            JsTsParser::detect_language("test.xyz_notareal_ext_9z8y".to_string(), "".to_string());
        assert_eq!(r.as_str(), "");
    }

    #[test]
    fn process_impl_empty_returns_valid_json() {
        let out = process_impl("", "javascript", "test.js");
        t::assert_valid_json(&out, "process(empty)");
    }

    #[test]
    fn process_impl_whitespace_returns_valid_json() {
        let out = process_impl("   \n  ", "javascript", "test.js");
        t::assert_valid_json(&out, "process(whitespace)");
    }

    // ---- issue #30: async-ness must survive into the tree, never style-only ----

    /// The whole defect in one assertion: the two sides must not serialise identically.
    fn assert_async_toggle_visible(plain: &str, asyncy: &str, language: &str, filename: &str) {
        let old = process_impl(plain, language, filename);
        let new = process_impl(asyncy, language, filename);
        assert_ne!(
            old, new,
            "async toggle produced byte-identical trees for {filename}: {plain:?} vs {asyncy:?}"
        );
    }

    #[test]
    fn async_toggle_changes_the_tree() {
        for (plain, asyncy) in [
            ("function f() { return 1; }", "async function f() { return 1; }"),
            ("const f = () => 1;", "const f = async () => 1;"),
            ("const f = function () { return 1; };", "const f = async function () { return 1; };"),
            ("class C { m() { return 1; } }", "class C { async m() { return 1; } }"),
            ("class C { static m() { return 1; } }", "class C { static async m() { return 1; } }"),
            ("function* g() { yield 1; }", "async function* g() { yield 1; }"),
        ] {
            assert_async_toggle_visible(plain, asyncy, "javascript", "test.js");
            assert_async_toggle_visible(plain, asyncy, "typescript", "test.ts");
        }
    }

    #[test]
    fn async_function_declaration_is_emitted_with_is_async_fact() {
        let out = process_impl("async function f() { return 1; }", "javascript", "test.js");
        assert!(
            out.contains("async_function_declaration"),
            "expected an async_function_declaration node, got: {out}"
        );
        assert!(
            out.contains("\"is_async\":true"),
            "expected the is_async NodeFacts flag, got: {out}"
        );
    }

    #[test]
    fn async_method_definition_is_emitted() {
        let out = process_impl("class C { async m() {} }", "javascript", "test.js");
        assert!(
            out.contains("async_method_definition"),
            "expected an async_method_definition node, got: {out}"
        );
    }

    /// `async` used as an ordinary name is an `identifier`, never the keyword token, so the
    /// scan in `async_variant_of` must not mint an async variant for it.
    #[test]
    fn async_as_a_plain_name_is_not_an_async_function() {
        let out = process_impl("function async() { return 1; }", "javascript", "test.js");
        assert!(
            !out.contains("async_function_declaration"),
            "`function async()` is not an async function, got: {out}"
        );
    }

    /// A plain function must keep its original node type and emit no facts.
    #[test]
    fn plain_function_is_untouched() {
        let out = process_impl("function f() { return 1; }", "javascript", "test.js");
        assert!(out.contains("function_declaration"), "got: {out}");
        assert!(!out.contains("async_"), "plain function gained async vocabulary: {out}");
        assert!(!out.contains("is_async"), "plain function gained an is_async fact: {out}");
    }
}
