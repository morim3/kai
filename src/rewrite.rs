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
    func_name: &str,
) -> String {
    let body_text = &source[reference_block.start_offset..reference_block.end_offset];

    // Determine the indentation of the original block from the source line.
    let original_indent = indent_at_offset(source, reference_block.start_offset);

    // Prepend the original indent to the body text so that all lines have
    // consistent indentation. The AST byte range starts at the first token,
    // so the first line's leading whitespace is missing from body_text.
    let full_body_text = format!("{original_indent}{body_text}");

    // Build the function body with parameter names substituted.
    // Replace the first block's variable names with the param/return names.
    let mut body = full_body_text;
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
    let mut func = format!("def {func_name}({params_str}):\n{body}\n");

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
/// and prepend the function definition. Returns the new source text.
///
/// The function definition is always generated from block 0 (the reference).
pub fn apply_refactoring(
    source: &str,
    blocks: &[MatchedBlock],
    sig: &FunctionSignature,
    func_name: &str,
) -> String {
    let func_def = generate_function_def(source, &blocks[0], sig, func_name);

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

    // Prepend the function definition.
    format!("{func_def}\n{result}")
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

/// Get the indentation of the line containing the given byte offset.
///
/// Unlike `detect_indent`, this works correctly for nested code because
/// it looks at the full source line, not just the AST byte range.
fn indent_at_offset(source: &str, offset: usize) -> String {
    let line_start = source[..offset].rfind('\n').map_or(0, |p| p + 1);
    source[line_start..offset].to_string()
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
        if i + old_chars.len() <= chars.len() && chars[i..i + old_chars.len()] == old_chars[..] {
            // Check word boundaries.
            let before_ok = i == 0 || !is_ident_char(chars[i - 1]);
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
    fn replace_identifier_whole_word() {
        assert_eq!(
            replace_identifier("x = x + xy", "x", "arg_0"),
            "arg_0 = arg_0 + xy"
        );
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
