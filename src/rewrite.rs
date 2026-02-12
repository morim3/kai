use std::collections::HashMap;

use ruff_python_ast::visitor::{Visitor, walk_expr};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::Ranged;
use similar::TextDiff;

use crate::SourcedBlock;
use crate::normalize::indent_at_offset;
use crate::scan::{MatchedBlock, ScopeContext, ScopeKind};
use crate::scope::FunctionSignature;

/// Byte position of an AST node (Name or literal) in source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodePosition {
    /// Byte offset from the start of the source.
    pub offset: usize,
    /// Byte length of the node's source text.
    pub len: usize,
}

/// Generate the extracted function definition as a string.
///
/// Uses the source text of the first matched block as the function body,
/// with variable names replaced according to the signature mapping.
/// `ref_node_positions` are pre-collected AST node positions from `collect_node_positions`.
/// The `scope` determines the base indentation of the generated function.
pub fn generate_function_def(
    source: &str,
    reference_block: &MatchedBlock,
    ref_node_positions: &[NodePosition],
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
    let rename_map = sig.rename_map();

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

/// Apply text edits to source, processing from end to start to preserve offsets.
///
/// Each edit is `(block_start_offset, block_end_offset, replacement_text)`.
/// Edits are extended to full line boundaries (consuming leading whitespace and trailing newline).
///
/// Safety: offsets are computed from `source` (immutable) and applied to `result`.
/// This is correct because edits are processed in descending offset order, so each
/// edit only modifies bytes at or above its range — lower offsets in `result` remain
/// identical to `source` when their turn comes.
fn apply_block_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut result = source.to_string();
    for (start, end, replacement) in &edits {
        let line_start = source[..*start].rfind('\n').map_or(0, |p| p + 1);
        let line_end = source[*end..].find('\n').map_or(*end, |p| *end + p + 1);
        debug_assert!(
            line_start <= line_end && line_end <= source.len(),
            "edit range out of bounds: {line_start}..{line_end}"
        );
        result.replace_range(line_start..line_end, replacement);
    }
    result
}

/// Build replacement edits for a set of matched blocks.
fn build_call_edits<'a>(
    source: &str,
    blocks: impl Iterator<Item = (usize, &'a MatchedBlock)>,
    sig: &FunctionSignature,
    func_name: &str,
) -> Vec<(usize, usize, String)> {
    blocks
        .map(|(sig_idx, block)| {
            let indent = indent_at_offset(source, block.start_offset);
            let call = generate_call(sig, sig_idx, func_name);
            let replacement = format!("{indent}{call}\n");
            (block.start_offset, block.end_offset, replacement)
        })
        .collect()
}

/// Apply the refactoring: replace matched blocks with function calls,
/// and insert the function definition at the appropriate scope.
///
/// `ref_node_positions` are pre-collected AST node positions for block 0 (the reference).
pub fn apply_refactoring(
    source: &str,
    blocks: &[MatchedBlock],
    ref_node_positions: &[NodePosition],
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

    let edits = build_call_edits(source, blocks.iter().enumerate(), sig, func_name);

    // Invariant: the function insertion point is always at or before the earliest
    // block edit. Since apply_block_edits processes edits end-to-start, bytes before
    // the earliest edit remain unchanged — so scope offsets from `source` are still
    // valid in `result`.
    let earliest_edit_offset = edits.iter().map(|(s, _, _)| *s).min().unwrap_or(0);
    let mut result = apply_block_edits(source, edits);

    match scope.kind {
        ScopeKind::Module => {
            // Prepend the function definition at the top of the file.
            format!("{func_def}\n{result}")
        }
        ScopeKind::Function => {
            debug_assert!(
                scope.body_start_offset <= earliest_edit_offset,
                "insertion point ({}) must be at or before earliest edit ({})",
                scope.body_start_offset,
                earliest_edit_offset
            );
            let insert_offset = result[..scope.body_start_offset]
                .rfind('\n')
                .map_or(0, |p| p + 1);
            result.insert_str(insert_offset, &format!("{func_def}\n"));
            result
        }
        ScopeKind::Class => {
            let class_offset = scope.class_def_offset.unwrap_or(0);
            debug_assert!(
                class_offset <= earliest_edit_offset,
                "class insertion point ({}) must be at or before earliest edit ({})",
                class_offset,
                earliest_edit_offset
            );
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

/// Generate an import statement: `from <module_stem> import <func_name>`.
pub fn generate_import(module_stem: &str, func_name: &str) -> String {
    format!("from {module_stem} import {func_name}\n")
}

/// Find the byte offset where a new import should be inserted.
///
/// Inserts after existing imports (the last `import` or `from ... import` line),
/// or at offset 0 if none exist.
fn find_import_insert_point(source: &str) -> usize {
    let mut last_import_end = None;
    for (offset, line) in line_offsets(source) {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
            last_import_end = Some(offset + line.len() + 1); // +1 for newline
        } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
            // Stop at first non-import, non-comment, non-blank line
            // (but only if we've seen at least one import or if this is code)
            if last_import_end.is_some() {
                break;
            }
        }
    }
    last_import_end.unwrap_or(0)
}

/// Iterate over lines with their byte offsets.
fn line_offsets(source: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut offset = 0;
    for line in source.lines() {
        result.push((offset, line));
        offset += line.len() + 1; // +1 for newline
    }
    result
}

/// Check if an import statement already exists in the source.
fn has_import(source: &str, module_stem: &str, func_name: &str) -> bool {
    let import_line = format!("from {module_stem} import {func_name}");
    source.lines().any(|line| line.trim() == import_line)
}

/// Apply the multi-file refactoring: returns one modified source per input file.
///
/// - `sources[0]` (target): gets the function definition + block replacements.
/// - `sources[1+]` (extra files): get block replacements + import insertion.
pub fn apply_refactoring_multi(
    sources: &[&str],
    blocks: &[SourcedBlock],
    ref_node_positions: &[NodePosition],
    sig: &FunctionSignature,
    func_name: &str,
    scope: &ScopeContext,
    target_file_stem: &str,
) -> Vec<String> {
    // Group blocks by source_index while preserving their position in the
    // original `blocks` slice (needed for block_arg_maps indexing).
    let mut per_file: Vec<Vec<(usize, &MatchedBlock)>> = vec![Vec::new(); sources.len()];
    for (block_idx, sourced) in blocks.iter().enumerate() {
        per_file[sourced.source_index].push((block_idx, &sourced.block));
    }

    let mut results = Vec::with_capacity(sources.len());

    for (file_idx, file_blocks) in per_file.iter().enumerate() {
        let source = sources[file_idx];

        if file_blocks.is_empty() {
            // No matches in this file — return unchanged.
            results.push(source.to_string());
            continue;
        }

        if file_idx == 0 {
            // Target file: use existing apply_refactoring.
            let plain_blocks: Vec<MatchedBlock> =
                file_blocks.iter().map(|(_, b)| (*b).clone()).collect();
            // We need to use the block_indices to construct proper sig for this file.
            // Since apply_refactoring uses sequential indexing, we need to create a
            // re-indexed signature for just this file's blocks.
            let file_sig = remap_signature(
                sig,
                &file_blocks.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            );
            results.push(apply_refactoring(
                source,
                &plain_blocks,
                ref_node_positions,
                &file_sig,
                func_name,
                scope,
            ));
        } else {
            // Extra file: replace blocks with calls + add import.
            let mut result = replace_blocks_with_calls(source, file_blocks, sig, func_name);

            // Add import if not already present.
            if !has_import(&result, target_file_stem, func_name) {
                let import_stmt = generate_import(target_file_stem, func_name);
                let insert_point = find_import_insert_point(&result);
                result.insert_str(insert_point, &import_stmt);
            }

            results.push(result);
        }
    }

    results
}

/// Replace matched blocks in a source file with function calls.
///
/// Used for non-target files in multi-file refactoring.
fn replace_blocks_with_calls(
    source: &str,
    blocks: &[(usize, &MatchedBlock)],
    sig: &FunctionSignature,
    func_name: &str,
) -> String {
    let edits = build_call_edits(
        source,
        blocks.iter().map(|&(idx, block)| (idx, block)),
        sig,
        func_name,
    );
    apply_block_edits(source, edits)
}

/// Create a new FunctionSignature with block_arg_maps/block_return_maps
/// re-indexed to only include the given block indices (in order).
fn remap_signature(sig: &FunctionSignature, block_indices: &[usize]) -> FunctionSignature {
    FunctionSignature {
        params: sig.params.clone(),
        returns: sig.returns.clone(),
        block_arg_maps: block_indices
            .iter()
            .map(|&i| sig.block_arg_maps[i].clone())
            .collect(),
        block_return_maps: block_indices
            .iter()
            .map(|&i| sig.block_return_maps[i].clone())
            .collect(),
    }
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
    node_positions: &[NodePosition],
    block_start: usize,
    indent_len: usize,
    rename_map: &HashMap<&str, &str>,
) -> String {
    // For each collected node, extract its source text and check the rename map.
    let mut replacements: Vec<(usize, usize, &str)> = Vec::new();
    for pos in node_positions {
        let original_text = &source[pos.offset..pos.offset + pos.len];
        if let Some(&new_name) = rename_map.get(original_text) {
            let body_offset = pos.offset - block_start + indent_len;
            replacements.push((body_offset, pos.len, new_name));
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
/// Returns owned `NodePosition` data that can outlive the AST borrow.
pub fn collect_node_positions(stmts: &[Stmt]) -> Vec<NodePosition> {
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
    positions: Vec<NodePosition>,
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
            self.positions.push(NodePosition { offset: start, len });
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

    #[test]
    fn generate_import_format() {
        assert_eq!(
            generate_import("utils", "compute"),
            "from utils import compute\n"
        );
    }

    #[test]
    fn find_import_insert_point_after_imports() {
        let source = "import os\nfrom sys import argv\n\nx = 1\n";
        let point = find_import_insert_point(source);
        // Should be after "from sys import argv\n"
        assert_eq!(point, "import os\nfrom sys import argv\n".len());
    }

    #[test]
    fn find_import_insert_point_no_imports() {
        let source = "x = 1\ny = 2\n";
        let point = find_import_insert_point(source);
        assert_eq!(point, 0);
    }

    #[test]
    fn has_import_detects_existing() {
        let source = "from utils import compute\nx = 1\n";
        assert!(has_import(source, "utils", "compute"));
        assert!(!has_import(source, "utils", "other_func"));
    }

    #[test]
    fn generate_function_def_basic() {
        use crate::scan::{MatchedBlock, ScopeContext, ScopeKind};
        use crate::test_utils::parse_stmts;

        let source = "a = 1\nb = a + 2\n";
        let block = MatchedBlock {
            start_line: 1,
            end_line: 2,
            start_offset: 0,
            end_offset: source.len() - 1, // exclude trailing newline
        };
        let stmts = parse_stmts(source);
        let positions = collect_node_positions(&stmts);
        let sig = make_sig(
            &["arg_0", "arg_1"],
            &[],
            &[&["a", "1"], &["x", "10"]],
            &[&[], &[]],
        );
        let scope = ScopeContext {
            kind: ScopeKind::Module,
            body_start_offset: 0,
            indent: String::new(),
            class_def_offset: None,
            parent_indent: None,
        };

        let result = generate_function_def(source, &block, &positions, &sig, "compute", &scope);
        assert_eq!(
            result,
            "def compute(arg_0, arg_1):\n    arg_0 = arg_1\n    b = arg_0 + 2\n"
        );
    }

    #[test]
    fn generate_function_def_with_returns() {
        use crate::scan::{MatchedBlock, ScopeContext, ScopeKind};
        use crate::test_utils::parse_stmts;

        let source = "result = x + 1\n";
        let block = MatchedBlock {
            start_line: 1,
            end_line: 1,
            start_offset: 0,
            end_offset: source.len() - 1,
        };
        let stmts = parse_stmts(source);
        let positions = collect_node_positions(&stmts);
        let sig = make_sig(
            &["arg_0"],
            &["ret_0"],
            &[&["x"], &["y"]],
            &[&["result"], &["output"]],
        );
        let scope = ScopeContext {
            kind: ScopeKind::Module,
            body_start_offset: 0,
            indent: String::new(),
            class_def_offset: None,
            parent_indent: None,
        };

        let result = generate_function_def(source, &block, &positions, &sig, "extract", &scope);
        assert_eq!(
            result,
            "def extract(arg_0):\n    ret_0 = arg_0 + 1\n    return ret_0\n"
        );
    }
}
