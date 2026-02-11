use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, MultiSelect};

use crate::rewrite;
use crate::scan::MatchedBlock;
use crate::scope::FunctionSignature;
use crate::{plan_extraction, scan};

// ── Validation ───────────────────────────────────────────────────────

/// Python keywords that cannot be used as identifiers.
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
    "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
    "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
    "try", "while", "with", "yield",
];

/// Check if a string is a valid Python identifier.
///
/// Rules: starts with letter or `_`, followed by letters/digits/`_`, not a keyword.
pub fn is_valid_python_ident(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    !PYTHON_KEYWORDS.contains(&name)
}

/// Validate a name and return an error message if invalid, or None if OK.
pub fn validate_ident(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Name cannot be empty".to_string());
    }
    if PYTHON_KEYWORDS.contains(&name) {
        return Some(format!("'{name}' is a Python keyword"));
    }
    if !is_valid_python_ident(name) {
        return Some(format!("'{name}' is not a valid Python identifier"));
    }
    None
}

/// Validate that the generated source is parseable Python.
pub fn validate_output(source: &str) -> Result<()> {
    ruff_python_parser::parse_module(source)
        .map_err(|e| anyhow::anyhow!("Generated code has syntax error: {e}"))?;
    Ok(())
}

// ── Display helpers ──────────────────────────────────────────────────

/// Format a block preview (first N chars of each line, joined by " / ").
fn block_preview(source: &str, block: &MatchedBlock, max_len: usize) -> String {
    let text = &source[block.start_offset..block.end_offset];
    let preview: String = text
        .lines()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" / ");
    if preview.len() > max_len {
        format!("{}...", &preview[..max_len])
    } else {
        preview
    }
}

// ── Interactive steps ────────────────────────────────────────────────

/// Step 1: Let the user select which matched blocks to include.
fn select_blocks(source: &str, blocks: &[MatchedBlock]) -> Result<Vec<usize>> {
    if blocks.len() <= 2 {
        return Ok((0..blocks.len()).collect());
    }

    eprintln!("\nFound {} matching blocks:", blocks.len());
    let items: Vec<String> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let preview = block_preview(source, b, 60);
            format!(
                "[{}] lines {}-{}: {}",
                i + 1,
                b.start_line,
                b.end_line,
                preview
            )
        })
        .collect();

    for item in &items {
        eprintln!("  {item}");
    }

    let defaults: Vec<bool> = vec![true; blocks.len()];
    let selections = MultiSelect::new()
        .with_prompt("Select blocks to extract [Space=toggle, Enter=confirm]")
        .items(&items)
        .defaults(&defaults)
        .interact()?;

    if selections.len() < 2 {
        bail!("Need at least 2 blocks selected for extraction.");
    }

    Ok(selections)
}

/// Step 2: Get a valid function name from the user.
fn get_function_name(default: &str) -> Result<String> {
    loop {
        let name: String = Input::new()
            .with_prompt("Function name")
            .default(default.to_string())
            .interact_text()?;
        if let Some(msg) = validate_ident(&name) {
            eprintln!("  Invalid: {msg}");
            continue;
        }
        return Ok(name);
    }
}

/// Step 3: Rename parameters with validation.
fn rename_parameters(sig: &mut FunctionSignature) -> Result<()> {
    if sig.params.is_empty() {
        return Ok(());
    }

    eprintln!("\nParameters (per-block values):");
    for (i, name) in sig.params.iter().enumerate() {
        let values: Vec<&str> = sig
            .block_arg_maps
            .iter()
            .map(|m| m.get(i).map(|s| s.as_str()).unwrap_or("?"))
            .collect();
        eprintln!("  {name}: {}", values.join(" | "));
    }

    for i in 0..sig.params.len() {
        loop {
            let current = &sig.params[i];
            let new_name: String = Input::new()
                .with_prompt(format!("Rename {current}"))
                .default(current.clone())
                .interact_text()?;

            if let Some(msg) = validate_ident(&new_name) {
                eprintln!("  Invalid: {msg}");
                continue;
            }

            // Check duplicates against other params.
            let dup = sig
                .params
                .iter()
                .enumerate()
                .any(|(j, p)| j != i && p == &new_name);
            if dup {
                eprintln!("  Invalid: '{new_name}' is already used by another parameter");
                continue;
            }

            if new_name != *current {
                sig.params[i] = new_name;
            }
            break;
        }
    }
    Ok(())
}

/// Step 4: Rename return values with validation.
fn rename_returns(sig: &mut FunctionSignature) -> Result<()> {
    if sig.returns.is_empty() {
        return Ok(());
    }

    eprintln!("\nReturns (per-block names):");
    for (i, ret_name) in sig.returns.iter().enumerate() {
        let values: Vec<&str> = sig
            .block_return_maps
            .iter()
            .map(|m| m.get(i).map(|s| s.as_str()).unwrap_or("?"))
            .collect();
        eprintln!("  {ret_name}: {}", values.join(" | "));
    }

    for i in 0..sig.returns.len() {
        loop {
            let current = sig.returns[i].clone();
            let new_name: String = Input::new()
                .with_prompt(format!("Rename {current}"))
                .default(current.clone())
                .interact_text()?;

            if let Some(msg) = validate_ident(&new_name) {
                eprintln!("  Invalid: {msg}");
                continue;
            }

            let dup = sig
                .returns
                .iter()
                .enumerate()
                .any(|(j, r)| j != i && r == &new_name);
            if dup {
                eprintln!("  Invalid: '{new_name}' is already used by another return value");
                continue;
            }

            if new_name != current {
                sig.returns[i] = new_name;
            }
            break;
        }
    }
    Ok(())
}

// ── Signature mutation utilities (public for testing) ────────────────

/// Remove parameters at indices NOT in `kept` from the signature.
pub fn remove_params(sig: &mut FunctionSignature, kept: &[usize]) {
    sig.params = kept.iter().map(|&i| sig.params[i].clone()).collect();
    for arg_map in &mut sig.block_arg_maps {
        *arg_map = kept
            .iter()
            .filter_map(|&i| arg_map.get(i).cloned())
            .collect();
    }
}

/// Remove return values at indices NOT in `kept` from the signature.
pub fn remove_returns(sig: &mut FunctionSignature, kept: &[usize]) {
    sig.returns = kept.iter().map(|&i| sig.returns[i].clone()).collect();
    for ret_map in &mut sig.block_return_maps {
        *ret_map = kept
            .iter()
            .filter_map(|&i| ret_map.get(i).cloned())
            .collect();
    }
}

// ── Main entry point ─────────────────────────────────────────────────

/// Run the interactive extraction workflow.
///
/// Flow: block selection → function name → param rename → return rename → preview.
pub fn run_interactive(
    source: &str,
    start_line: usize,
    end_line: usize,
    file_path: Option<&str>,
    show_diff: bool,
) -> Result<()> {
    // Stage 1: Scan for matches.
    let all_blocks = scan::find_matches(source, start_line, end_line)?;
    if all_blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            all_blocks.len()
        );
    }

    // Step 1: Block selection.
    let selected_indices = select_blocks(source, &all_blocks)?;
    let blocks: Vec<MatchedBlock> = selected_indices
        .iter()
        .map(|&i| all_blocks[i].clone())
        .collect();

    // Stage 2: Compute extraction plan.
    let plan = plan_extraction(source, &blocks, start_line, end_line)?;
    let mut sig = plan.sig;

    // Step 2: Function name.
    let func_name = get_function_name("extracted_func_0")?;

    // Step 3: Parameter rename.
    rename_parameters(&mut sig)?;

    // Step 4: Return value rename.
    rename_returns(&mut sig)?;

    // Stage 3: Apply refactoring.
    let result = rewrite::apply_refactoring(
        source,
        &blocks,
        &plan.ref_node_positions,
        &sig,
        &func_name,
        &plan.scope_ctx,
    );

    // Final safety check: ensure generated code is valid Python.
    validate_output(&result)?;

    // Step 5: Preview and confirm.
    if show_diff {
        let filename = file_path.unwrap_or("<stdin>");
        let diff = rewrite::unified_diff(source, &result, filename);
        eprintln!("\n--- Preview (diff) ---");
        print!("{diff}");
    } else {
        eprintln!("\n--- Preview ---");
        print!("{result}");
    }

    if let Some(path) = file_path {
        let write = Confirm::new()
            .with_prompt("Write to file?")
            .default(false)
            .interact()?;
        if write {
            std::fs::write(path, &result)?;
            eprintln!("Wrote refactored code to {path}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── Validation tests ──

    #[test]
    fn valid_idents() {
        assert!(is_valid_python_ident("foo"));
        assert!(is_valid_python_ident("_bar"));
        assert!(is_valid_python_ident("x1"));
        assert!(is_valid_python_ident("arg_0"));
    }

    #[test]
    fn invalid_idents() {
        // Empty
        assert!(!is_valid_python_ident(""));
        // Starts with digit
        assert!(!is_valid_python_ident("123x"));
        // Contains hyphen
        assert!(!is_valid_python_ident("my-var"));
        // Contains space
        assert!(!is_valid_python_ident("my var"));
        // Python keyword
        assert!(!is_valid_python_ident("if"));
        assert!(!is_valid_python_ident("return"));
        assert!(!is_valid_python_ident("class"));
    }

    #[test]
    fn validate_ident_messages() {
        assert!(validate_ident("good_name").is_none());
        assert!(validate_ident("").unwrap().contains("empty"));
        assert!(validate_ident("if").unwrap().contains("keyword"));
        assert!(validate_ident("1bad").unwrap().contains("not a valid"));
    }

    #[test]
    fn validate_output_accepts_valid_python() {
        assert!(validate_output("x = 1\n").is_ok());
    }

    #[test]
    fn validate_output_rejects_invalid_python() {
        assert!(validate_output("def (broken\n").is_err());
    }

    // ── remove_params / remove_returns tests ──

    #[test]
    fn remove_params_keeps_selected() {
        let mut sig = make_sig(
            &["arg_0", "arg_1", "arg_2"],
            &["ret_0"],
            &[&["a", "1", "x"], &["b", "10", "y"]],
            &[&["c"], &["z"]],
        );
        remove_params(&mut sig, &[0, 2]);
        assert_eq!(sig.params, vec!["arg_0", "arg_2"]);
        assert_eq!(sig.block_arg_maps[0], vec!["a", "x"]);
        assert_eq!(sig.block_arg_maps[1], vec!["b", "y"]);
    }

    #[test]
    fn remove_params_all_kept() {
        let mut sig = make_sig(&["arg_0", "arg_1"], &[], &[&["a", "1"]], &[&[]]);
        remove_params(&mut sig, &[0, 1]);
        assert_eq!(sig.params, vec!["arg_0", "arg_1"]);
        assert_eq!(sig.block_arg_maps[0], vec!["a", "1"]);
    }

    #[test]
    fn remove_params_empty() {
        let mut sig = make_sig(&[], &[], &[&[]], &[&[]]);
        remove_params(&mut sig, &[]);
        assert!(sig.params.is_empty());
    }

    #[test]
    fn remove_returns_keeps_selected() {
        let mut sig = make_sig(
            &["arg_0"],
            &["ret_0", "ret_1"],
            &[&["a"], &["b"]],
            &[&["c", "d"], &["z", "w"]],
        );
        remove_returns(&mut sig, &[1]);
        assert_eq!(sig.returns, vec!["ret_1"]);
        assert_eq!(sig.block_return_maps[0], vec!["d"]);
        assert_eq!(sig.block_return_maps[1], vec!["w"]);
    }

    #[test]
    fn remove_returns_all_removed() {
        let mut sig = make_sig(
            &["arg_0"],
            &["ret_0"],
            &[&["a"], &["b"]],
            &[&["c"], &["z"]],
        );
        remove_returns(&mut sig, &[]);
        assert!(sig.returns.is_empty());
        assert!(sig.block_return_maps[0].is_empty());
        assert!(sig.block_return_maps[1].is_empty());
    }

    // ── Simulated pipeline tests ──

    #[test]
    fn simulated_interactive_matches_non_interactive() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
e = 100
f = e + 200
";
        let expected = crate::extract_method(source, 1, 2).unwrap();

        let all_blocks = scan::find_matches(source, 1, 2).unwrap();
        assert_eq!(all_blocks.len(), 3);

        let plan = crate::plan_extraction(source, &all_blocks, 1, 2).unwrap();
        let result = rewrite::apply_refactoring(
            source,
            &all_blocks,
            &plan.ref_node_positions,
            &plan.sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn simulated_interactive_block_subset() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
e = 100
f = e + 200
";
        let all_blocks = scan::find_matches(source, 1, 2).unwrap();
        let blocks: Vec<MatchedBlock> = vec![all_blocks[0].clone(), all_blocks[1].clone()];
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &plan.sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );

        assert!(result.contains("e = 100"));
        assert!(result.contains("f = e + 200"));
        assert!(result.contains("extracted_func_0(1, 2)"));
        assert!(result.contains("extracted_func_0(10, 20)"));
        assert!(result.contains("def extracted_func_0(arg_0, arg_1):"));
    }

    #[test]
    fn simulated_interactive_param_rename() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        sig.params[0] = "value".to_string();
        sig.params[1] = "offset".to_string();

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "compute",
            &plan.scope_ctx,
        );
        assert!(result.contains("def compute(value, offset):"));
        assert!(validate_output(&result).is_ok());
    }

    #[test]
    fn simulated_interactive_return_rename() {
        let source = "\
a = 1
b = a + 2
print(b)
c = 10
d = c + 20
print(d)
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        if !sig.returns.is_empty() {
            sig.returns[0] = "output".to_string();
        }

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );
        if !sig.returns.is_empty() {
            assert!(result.contains("return output"));
        }
        assert!(validate_output(&result).is_ok());
    }

    /// Verify generated output is always valid Python even with custom names.
    #[test]
    fn generated_output_always_valid_python() {
        let source = "\
a = 1
b = a + 2
print(b)
c = 10
d = c + 20
print(d)
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();

        // Default names
        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &plan.sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );
        assert!(validate_output(&result).is_ok(), "default names must be valid");

        // Custom names
        let mut sig = plan.sig.clone();
        sig.params[0] = "x".to_string();
        if !sig.returns.is_empty() {
            sig.returns[0] = "result".to_string();
        }
        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "my_func",
            &plan.scope_ctx,
        );
        assert!(validate_output(&result).is_ok(), "custom names must be valid");
    }

    #[test]
    fn block_preview_truncates() {
        let source =
            "long_variable_name = some_very_long_function_call(with_lots_of_arguments, and_more)\n";
        let block = MatchedBlock {
            start_line: 1,
            end_line: 1,
            start_offset: 0,
            end_offset: source.len() - 1,
        };
        let preview = block_preview(source, &block, 30);
        assert!(preview.len() <= 33); // 30 + "..."
        assert!(preview.ends_with("..."));
    }
}
