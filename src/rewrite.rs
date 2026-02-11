use similar::TextDiff;

use crate::scan::MatchedBlock;
use crate::scope::FunctionSignature;

/// Generate the extracted function definition as a string.
///
/// Uses the source text of the first matched block as the function body,
/// with variable names replaced according to the signature mapping.
pub fn generate_function_def(
    source: &str,
    reference_block: &MatchedBlock,
    sig: &FunctionSignature,
) -> String {
    let body_text = &source[reference_block.start_offset..reference_block.end_offset];

    // Determine the indentation of the original block.
    let original_indent = detect_indent(body_text);

    // Build the function body with parameter names substituted.
    // Replace the first block's variable names with the param/return names.
    let mut body = body_text.to_string();
    if let Some(arg_map) = sig.block_arg_maps.first() {
        for (i, original_name) in arg_map.iter().enumerate() {
            body = replace_identifier(&body, original_name, &sig.params[i]);
        }
    }
    if let Some(ret_map) = sig.block_return_maps.first() {
        for (i, original_name) in ret_map.iter().enumerate() {
            body = replace_identifier(&body, original_name, &sig.returns[i]);
        }
    }

    // Re-indent the body to 4 spaces (function body indent).
    let body = reindent(&body, &original_indent, "    ");

    // Build the function definition.
    let params_str = sig.params.join(", ");
    let mut func = format!("def extracted_func_0({params_str}):\n{body}\n");

    // Add return statement if there are outputs.
    if !sig.returns.is_empty() {
        let return_expr = sig.returns.join(", ");
        func.push_str(&format!("    return {return_expr}\n"));
    }

    func
}

/// Generate the replacement call for a matched block.
///
/// `block_index` selects which block's variable mapping to use.
pub fn generate_call(sig: &FunctionSignature, block_index: usize) -> String {
    let args: &[String] = &sig.block_arg_maps[block_index];
    let args_str = args.join(", ");
    let call = format!("extracted_func_0({args_str})");

    if sig.returns.is_empty() {
        call
    } else {
        let targets: &[String] = &sig.block_return_maps[block_index];
        let targets_str = targets.join(", ");
        format!("{targets_str} = {call}")
    }
}

/// Apply the refactoring: replace all matched blocks with function calls,
/// and prepend the function definition. Returns the new source text.
pub fn apply_refactoring(
    source: &str,
    blocks: &[MatchedBlock],
    sig: &FunctionSignature,
) -> String {
    let func_def = generate_function_def(source, &blocks[0], sig);

    // Build edits sorted by offset (descending so we can apply from end to start).
    let mut edits: Vec<(usize, usize, String)> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            let indent = detect_indent(&source[block.start_offset..block.end_offset]);
            let call = generate_call(sig, i);
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

    // Prepend the function definition.
    format!("{func_def}\n{result}")
}

/// Generate a unified diff between original and new source.
pub fn unified_diff(original: &str, modified: &str, filename: &str) -> String {
    let diff = TextDiff::from_lines(original, modified);
    let mut output = String::new();
    for hunk in diff.unified_diff().header(&format!("a/{filename}"), &format!("b/{filename}")).iter_hunks() {
        output.push_str(&format!("{hunk}"));
    }
    output
}

/// Detect the leading whitespace (indent) of the first line in a code block.
fn detect_indent(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("");
    let trimmed = first_line.trim_start();
    first_line[..first_line.len() - trimmed.len()].to_string()
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

/// Replace whole-word occurrences of `old_name` with `new_name` in Python source.
/// Simple word-boundary replacement (not full AST-based, but sufficient for identifiers).
fn replace_identifier(source: &str, old_name: &str, new_name: &str) -> String {
    if old_name == new_name {
        return source.to_string();
    }
    let mut result = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let old_chars: Vec<char> = old_name.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + old_chars.len() <= chars.len()
            && chars[i..i + old_chars.len()] == old_chars[..]
        {
            // Check word boundaries.
            let before_ok =
                i == 0 || !is_ident_char(chars[i - 1]);
            let after_ok =
                i + old_chars.len() >= chars.len() || !is_ident_char(chars[i + old_chars.len()]);

            if before_ok && after_ok {
                result.push_str(new_name);
                i += old_chars.len();
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::FunctionSignature;

    fn make_sig(
        params: &[&str],
        returns: &[&str],
        arg_maps: &[&[&str]],
        ret_maps: &[&[&str]],
    ) -> FunctionSignature {
        FunctionSignature {
            params: params.iter().map(|s| s.to_string()).collect(),
            returns: returns.iter().map(|s| s.to_string()).collect(),
            block_arg_maps: arg_maps
                .iter()
                .map(|m| m.iter().map(|s| s.to_string()).collect())
                .collect(),
            block_return_maps: ret_maps
                .iter()
                .map(|m| m.iter().map(|s| s.to_string()).collect())
                .collect(),
        }
    }

    fn make_block(start_line: usize, end_line: usize, start: usize, end: usize) -> MatchedBlock {
        MatchedBlock {
            start_line,
            end_line,
            start_offset: start,
            end_offset: end,
        }
    }

    #[test]
    fn generate_call_no_returns() {
        let sig = make_sig(&["arg_0", "arg_1"], &[], &[&["x", "y"]], &[&[]]);
        let call = generate_call(&sig, 0);
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
        assert_eq!(generate_call(&sig, 0), "result = extracted_func_0(x)");
        assert_eq!(generate_call(&sig, 1), "output = extracted_func_0(a)");
    }

    #[test]
    fn replace_identifier_whole_word() {
        assert_eq!(
            replace_identifier("x = x + xy", "x", "arg_0"),
            "arg_0 = arg_0 + xy"
        );
    }

    #[test]
    fn end_to_end_refactoring() {
        use crate::scan::find_matches;
        use crate::scope::unify_signatures;
        use ruff_python_parser::parse_module;

        let source = "\
a = 1
b = a + 2
c = 3
x = 100
y = x + 200
c = 3
";
        // Use the real pipeline to get correct offsets.
        let blocks = find_matches(source, 1, 2).unwrap();
        assert_eq!(blocks.len(), 2);

        // Build scope info for each block.
        let parsed = parse_module(source).unwrap();
        let body = &parsed.into_syntax().body;

        let mut sig_inputs: Vec<(&[ruff_python_ast::Stmt], &[ruff_python_ast::Stmt])> = Vec::new();
        for block in &blocks {
            // Find the statement indices for this block.
            let block_stmts_start = body.iter().position(|s| {
                use ruff_text_size::Ranged;
                s.range().start().to_usize() == block.start_offset
            }).unwrap();
            let window_size = 2; // we know from the target
            let after_start = block_stmts_start + window_size;
            let block_slice = &body[block_stmts_start..block_stmts_start + window_size];
            let after_slice = if after_start < body.len() {
                &body[after_start..]
            } else {
                &[]
            };
            sig_inputs.push((block_slice, after_slice));
        }

        // Extract divergences between blocks.
        let mut all_divs = Vec::new();
        if sig_inputs.len() >= 2 {
            let (ref_block, _) = &sig_inputs[0];
            for (other_block, _) in sig_inputs.iter().skip(1) {
                let divs = crate::diff_extract::extract_divergences(ref_block, other_block, source, source);
                all_divs.push(divs);
            }
        }

        let sig = unify_signatures(&sig_inputs, &all_divs);
        let result = apply_refactoring(source, &blocks, &sig);

        // The rewritten code must parse as valid Python.
        let re_parsed = ruff_python_parser::parse_module(&result);
        assert!(
            re_parsed.is_ok(),
            "Rewritten code must be valid Python. Got:\n{result}"
        );

        // Check that the diff is non-empty.
        let diff = unified_diff(source, &result, "test.py");
        assert!(!diff.is_empty(), "Diff should not be empty");
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
    fn detect_indent_works() {
        assert_eq!(detect_indent("    x = 1\n    y = 2"), "    ");
        assert_eq!(detect_indent("x = 1"), "");
    }

    #[test]
    fn reindent_works() {
        let text = "    x = 1\n    y = 2";
        let result = reindent(text, "    ", "        ");
        assert_eq!(result, "        x = 1\n        y = 2");
    }
}
