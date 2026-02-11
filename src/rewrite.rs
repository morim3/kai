use std::collections::HashMap;

use ruff_python_ast::visitor::{Visitor, walk_expr};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::Ranged;
use similar::TextDiff;

use crate::normalize::indent_at_offset;
use crate::scan::{MatchedBlock, ScopeContext, ScopeKind};
use crate::scope::FunctionSignature;

/// Generate the extracted function definition as a string.
///
/// Uses the source text of the first matched block as the function body,
/// with variable names replaced according to the signature mapping.
/// `ref_node_positions` are pre-collected AST node positions from `collect_node_positions`.
/// The `scope` determines the base indentation of the generated function.
pub fn generate_function_def(
    source: &str,
    reference_block: &MatchedBlock,
    ref_node_positions: &[(usize, usize)],
    sig: &FunctionSignature,
    func_name: &str,
    scope: &ScopeContext,
) -> String {
    let body_text = &source[reference_block.start_offset..reference_block.end_offset];

    // Determine the indentation of the original block from the source line.
    let original_indent = indent_at_offset(source, reference_block.start_offset);

    // Prepend the original indent to the body text so that all lines have
    // consistent indentation. The AST byte range starts at the first token,
    // so the first line's leading whitespace is missing from body_text.
    let full_body_text = format!("{original_indent}{body_text}");

    // Build rename map from original names → param/return names.
    let mut rename_map: HashMap<&str, &str> = HashMap::new();
    if let Some(arg_map) = sig.block_arg_maps.first() {
        for (i, original_name) in arg_map.iter().enumerate() {
            rename_map.insert(original_name, &sig.params[i]);
        }
    }
    if let Some(ret_map) = sig.block_return_maps.first() {
        for (i, original_name) in ret_map.iter().enumerate() {
            rename_map.insert(original_name, &sig.returns[i]);
        }
    }

    // Use pre-collected AST node positions for precise replacement (avoids
    // false matches in string literals and comments).
    let body = replace_names_ast(
        &full_body_text,
        source,
        ref_node_positions,
        reference_block.start_offset,
        original_indent.len(),
        &rename_map,
    );

    // For Class scope, the function is placed outside the class at the parent indent.
    let def_indent = if scope.kind == ScopeKind::Class {
        scope.parent_indent.as_deref().unwrap_or("")
    } else {
        &scope.indent
    };

    // Determine the function body indent (one level deeper than the def line).
    let body_indent = format!("{def_indent}    ");

    // Re-indent the body to the correct level.
    let body = reindent(&body, &original_indent, &body_indent);

    let params_str = sig.params.join(", ");
    let mut func = format!("{def_indent}def {func_name}({params_str}):\n{body}\n");

    // Add return statement if there are outputs.
    if !sig.returns.is_empty() {
        let return_expr = sig.returns.join(", ");
        func.push_str(&format!("{body_indent}return {return_expr}\n"));
    }

    func
}

/// Generate the replacement call for a matched block.
///
/// `block_index` selects which block's variable mapping to use.
pub fn generate_call(sig: &FunctionSignature, block_index: usize, func_name: &str) -> String {
    let args: &[String] = &sig.block_arg_maps[block_index];
    let args_str = args.join(", ");
    let call = format!("{func_name}({args_str})");

    if sig.returns.is_empty() {
        call
    } else {
        let targets: &[String] = &sig.block_return_maps[block_index];
        let targets_str = targets.join(", ");
        format!("{targets_str} = {call}")
    }
}

/// Apply the refactoring: replace matched blocks with function calls,
/// and insert the function definition at the appropriate scope.
///
/// `ref_node_positions` are pre-collected AST node positions for block 0 (the reference).
pub fn apply_refactoring(
    source: &str,
    blocks: &[MatchedBlock],
    ref_node_positions: &[(usize, usize)],
    sig: &FunctionSignature,
    func_name: &str,
    scope: &ScopeContext,
) -> String {
    let func_def = generate_function_def(
        source,
        &blocks[0],
        ref_node_positions,
        sig,
        func_name,
        scope,
    );

    // Build edits sorted by offset (descending so we can apply from end to start).
    let mut edits: Vec<(usize, usize, String)> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            let indent = indent_at_offset(source, block.start_offset);
            let call = generate_call(sig, i, func_name);
            let replacement = format!("{indent}{call}\n");
            (block.start_offset, block.end_offset, replacement)
        })
        .collect();

    // Sort descending by start offset so edits don't invalidate each other.
    edits.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result = source.to_string();
    for (start, end, replacement) in &edits {
        // Extend to include the full line (eat leading whitespace and trailing newline).
        let line_start = source[..*start].rfind('\n').map_or(0, |p| p + 1);
        let line_end = source[*end..].find('\n').map_or(*end, |p| *end + p + 1);
        result.replace_range(line_start..line_end, replacement);
    }

    match scope.kind {
        ScopeKind::Module => {
            // Prepend the function definition at the top of the file.
            format!("{func_def}\n{result}")
        }
        ScopeKind::Function => {
            // Insert at the beginning of the scope body.
            let insert_offset = result[..scope.body_start_offset]
                .rfind('\n')
                .map_or(0, |p| p + 1);
            result.insert_str(insert_offset, &format!("{func_def}\n"));
            result
        }
        ScopeKind::Class => {
            // Insert before the class definition.
            let class_offset = scope.class_def_offset.unwrap_or(0);
            let insert_offset = result[..class_offset].rfind('\n').map_or(0, |p| p + 1);
            result.insert_str(insert_offset, &format!("{func_def}\n"));
            result
        }
    }
}

/// Generate a unified diff between original and new source.
pub fn unified_diff(original: &str, modified: &str, filename: &str) -> String {
    let diff = TextDiff::from_lines(original, modified);
    let mut output = String::new();
    for hunk in diff
        .unified_diff()
        .header(&format!("a/{filename}"), &format!("b/{filename}"))
        .iter_hunks()
    {
        output.push_str(&format!("{hunk}"));
    }
    output
}

/// Re-indent a code block from `old_indent` to `new_indent`.
fn reindent(text: &str, old_indent: &str, new_indent: &str) -> String {
    text.lines()
        .map(|line| {
            if let Some(stripped) = line.strip_prefix(old_indent) {
                format!("{new_indent}{stripped}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace variable names and literals in `body_text` using pre-collected AST node positions.
///
/// Only replaces at positions where the parser identified actual Name or Literal
/// tokens, avoiding false matches in string literals and comments.
///
/// * `body_text` — the block text with leading indent prepended.
/// * `source` — the original full source (used to extract text at AST positions).
/// * `node_positions` — pre-collected `(byte_offset, byte_len)` from `collect_node_positions`.
/// * `block_start` — byte offset of the block in the original source.
/// * `indent_len` — length of the prepended indent (shifts offsets in body_text).
/// * `rename_map` — maps original text (variable name or literal) → new name.
fn replace_names_ast(
    body_text: &str,
    source: &str,
    node_positions: &[(usize, usize)],
    block_start: usize,
    indent_len: usize,
    rename_map: &HashMap<&str, &str>,
) -> String {
    // For each collected node, extract its source text and check the rename map.
    let mut replacements: Vec<(usize, usize, &str)> = Vec::new();
    for &(src_offset, len) in node_positions {
        let original_text = &source[src_offset..src_offset + len];
        if let Some(&new_name) = rename_map.get(original_text) {
            let body_offset = src_offset - block_start + indent_len;
            replacements.push((body_offset, len, new_name));
        }
    }

    // Sort descending by offset so replacements don't invalidate each other.
    replacements.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result = body_text.to_string();
    for (offset, len, new_name) in &replacements {
        result.replace_range(*offset..*offset + *len, new_name);
    }
    result
}

/// Collect byte positions of all `Expr::Name` and literal nodes from AST statements.
///
/// Returns `Vec<(source_byte_offset, byte_length)>` as owned data that can
/// outlive the AST borrow.
pub fn collect_node_positions(stmts: &[Stmt]) -> Vec<(usize, usize)> {
    let mut collector = NodeCollector {
        positions: Vec::new(),
    };
    for stmt in stmts {
        collector.visit_stmt(stmt);
    }
    collector.positions
}

/// Collects byte positions of all `Expr::Name` and literal nodes in an AST subtree.
struct NodeCollector {
    /// (source_byte_offset, byte_length)
    positions: Vec<(usize, usize)>,
}

impl<'a> Visitor<'a> for NodeCollector {
    fn visit_expr(&mut self, expr: &'a Expr) {
        let collect = matches!(
            expr,
            Expr::Name(_)
                | Expr::NumberLiteral(_)
                | Expr::StringLiteral(_)
                | Expr::BytesLiteral(_)
                | Expr::BooleanLiteral(_)
        );
        if collect {
            let range = expr.range();
            let start = range.start().to_usize();
            let len = range.end().to_usize() - start;
            self.positions.push((start, len));
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_sig;

    #[test]
    fn generate_call_no_returns() {
        let sig = make_sig(&["arg_0", "arg_1"], &[], &[&["x", "y"]], &[&[]]);
        let call = generate_call(&sig, 0, "extracted_func_0");
        assert_eq!(call, "extracted_func_0(x, y)");
    }

    #[test]
    fn generate_call_with_returns() {
        let sig = make_sig(
            &["arg_0"],
            &["ret_0"],
            &[&["x"], &["a"]],
            &[&["result"], &["output"]],
        );
        assert_eq!(
            generate_call(&sig, 0, "extracted_func_0"),
            "result = extracted_func_0(x)"
        );
        assert_eq!(
            generate_call(&sig, 1, "extracted_func_0"),
            "output = extracted_func_0(a)"
        );
    }

    #[test]
    fn generate_call_custom_name() {
        let sig = make_sig(&["x", "y"], &[], &[&["a", "b"]], &[&[]]);
        let call = generate_call(&sig, 0, "compute");
        assert_eq!(call, "compute(a, b)");
    }

    #[test]
    fn replace_names_ast_skips_strings() {
        use crate::test_utils::parse_stmts;

        let source = "a = \"hello a world\"\nb = a + 1\n";
        let stmts = parse_stmts(source);
        let positions = collect_node_positions(&stmts);
        let mut rename_map = HashMap::new();
        rename_map.insert("a", "arg_0");

        // body_text == source (no prepend), block_start = 0, indent_len = 0
        let result = replace_names_ast(source, source, &positions, 0, 0, &rename_map);
        // The 'a' in the string literal must NOT be replaced.
        assert_eq!(result, "arg_0 = \"hello a world\"\nb = arg_0 + 1\n");
    }

    #[test]
    fn unified_diff_output() {
        let original = "a = 1\nb = 2\n";
        let modified = "a = 1\nb = 3\n";
        let diff = unified_diff(original, modified, "test.py");
        assert!(diff.contains("-b = 2"), "Diff should show removed line");
        assert!(diff.contains("+b = 3"), "Diff should show added line");
    }

    #[test]
    fn indent_at_offset_works() {
        let source = "    x = 1\n        y = 2\n";
        // offset 4 is at 'x', indent is "    "
        assert_eq!(indent_at_offset(source, 4), "    ");
        // offset 14 is at 'y' (after "    x = 1\n        "), indent is "        "
        assert_eq!(indent_at_offset(source, 18), "        ");
        // offset 0 at start of file
        assert_eq!(indent_at_offset("x = 1", 0), "");
    }

    #[test]
    fn reindent_works() {
        let text = "    x = 1\n    y = 2";
        let result = reindent(text, "    ", "        ");
        assert_eq!(result, "        x = 1\n        y = 2");
    }
}
