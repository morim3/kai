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

/// Generic rename loop: display per-block values, then prompt the user to rename
/// each item with validation and duplicate checking.
///
/// `reserved` contains names from other collections (e.g. params when renaming
/// returns) that must not be reused, preventing cross-collection collisions.
fn rename_collection(
    names: &mut [String],
    per_block_maps: &[Vec<String>],
    reserved: &[String],
    header: &str,
    label: &str,
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }

    eprintln!("\n{header}:");
    for (i, name) in names.iter().enumerate() {
        let values: Vec<&str> = per_block_maps
            .iter()
            .map(|m| m.get(i).map(|s| s.as_str()).unwrap_or("?"))
            .collect();
        eprintln!("  {name}: {}", values.join(" | "));
    }

    for i in 0..names.len() {
        loop {
            let current = names[i].clone();
            let new_name: String = Input::new()
                .with_prompt(format!("Rename {current}"))
                .default(current.clone())
                .interact_text()?;

            if let Some(msg) = validate_ident(&new_name) {
                eprintln!("  Invalid: {msg}");
                continue;
            }

            // Check duplicates within this collection.
            let dup_self = names
                .iter()
                .enumerate()
                .any(|(j, n)| j != i && n == &new_name);
            // Check collisions with reserved names from other collections.
            let dup_reserved = reserved.iter().any(|r| r == &new_name);
            if dup_self || dup_reserved {
                eprintln!("  Invalid: '{new_name}' is already used by another {label}");
                continue;
            }

            if new_name != current {
                names[i] = new_name;
            }
            break;
        }
    }
    Ok(())
}

/// Step 3: Rename parameters with validation.
fn rename_parameters(sig: &mut FunctionSignature) -> Result<()> {
    rename_collection(
        &mut sig.params,
        &sig.block_arg_maps,
        &[],
        "Parameters (per-block values)",
        "parameter",
    )
}

/// Step 4: Rename return values with validation.
fn rename_returns(sig: &mut FunctionSignature) -> Result<()> {
    rename_collection(
        &mut sig.returns,
        &sig.block_return_maps,
        &[],
        "Returns (per-block names)",
        "return value",
    )
}

/// Step 5: Offer additional return value candidates from block stores.
///
/// Shows variables stored in the block that are NOT already returns.
/// User can select additional ones to include as return values.
fn add_returns(
    sig: &mut FunctionSignature,
    block_stores: &[Vec<String>],
) -> Result<()> {
    if block_stores.is_empty() {
        return Ok(());
    }

    // Candidates: variables stored in block 0 that are not already returns.
    let existing: std::collections::HashSet<&str> = sig
        .block_return_maps
        .first()
        .map(|m| m.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let ref_stores = &block_stores[0];
    let candidate_indices: Vec<usize> = ref_stores
        .iter()
        .enumerate()
        .filter(|(_, name)| !existing.contains(name.as_str()))
        .map(|(i, _)| i)
        .collect();

    if candidate_indices.is_empty() {
        return Ok(());
    }

    let items: Vec<String> = candidate_indices
        .iter()
        .map(|&i| {
            let values: Vec<&str> = block_stores
                .iter()
                .map(|stores| stores.get(i).map(|s| s.as_str()).unwrap_or("?"))
                .collect();
            format!("{}: {}", ref_stores[i], values.join(" | "))
        })
        .collect();

    eprintln!("\nAdditional return candidates (variables stored in block):");
    for item in &items {
        eprintln!("  [ ] {item}");
    }

    let defaults: Vec<bool> = vec![false; items.len()];
    let selections = MultiSelect::new()
        .with_prompt("Add return values [Space=toggle, Enter=confirm]")
        .items(&items)
        .defaults(&defaults)
        .interact()?;

    if selections.is_empty() {
        return Ok(());
    }

    // Add selected stores to sig.returns and block_return_maps.
    let ret_count = sig.returns.len();
    for (sel_idx, &sel) in selections.iter().enumerate() {
        let store_idx = candidate_indices[sel];
        let ret_name = format!("ret_{}", ret_count + sel_idx);
        sig.returns.push(ret_name);

        for (block_idx, ret_map) in sig.block_return_maps.iter_mut().enumerate() {
            let var_name = block_stores
                .get(block_idx)
                .and_then(|s| s.get(store_idx))
                .cloned()
                .unwrap_or_default();
            ret_map.push(var_name);
        }
    }

    // Rename newly added returns (reserved = existing returns only).
    let reserved: Vec<String> = sig.returns[..ret_count].to_vec();
    let new_returns = &mut sig.returns[ret_count..];
    let new_maps: Vec<Vec<String>> = sig
        .block_return_maps
        .iter()
        .map(|m| m[ret_count..].to_vec())
        .collect();
    rename_collection(new_returns, &new_maps, &reserved, "Rename added returns", "return value")?;

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

    // Step 5: Add additional return values.
    add_returns(&mut sig, &plan.block_stores)?;

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
    use crate::test_utils::make_sig;

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

    // ── add_returns logic tests (unit-level, no TTY) ──

    /// Verify that add_returns correctly modifies the signature when called
    /// with pre-computed selections (simulating the MultiSelect result).
    #[test]
    fn add_returns_logic_adds_store_variables() {
        // Simulate: 2 blocks, each stores a, b.  Returns already has "b".
        // Candidate should be "a" only.  We simulate adding it.
        let mut sig = make_sig(
            &["arg_0"],
            &["ret_0"],
            &[&["x"], &["y"]],
            &[&["b"], &["d"]],
        );
        let block_stores = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string()],
        ];

        // Manually invoke the logic that add_returns would do (skipping MultiSelect).
        let existing: std::collections::HashSet<&str> = sig
            .block_return_maps[0]
            .iter()
            .map(|s| s.as_str())
            .collect();
        let ref_stores = &block_stores[0];
        let candidate_indices: Vec<usize> = ref_stores
            .iter()
            .enumerate()
            .filter(|(_, name)| !existing.contains(name.as_str()))
            .map(|(i, _)| i)
            .collect();

        // "a" at index 0 is the only candidate (b is already a return).
        assert_eq!(candidate_indices, vec![0]);

        // Simulate user selecting the candidate.
        let selections = vec![0usize]; // index into candidate_indices
        let ret_count = sig.returns.len();
        for (sel_idx, &sel) in selections.iter().enumerate() {
            let store_idx = candidate_indices[sel];
            let ret_name = format!("ret_{}", ret_count + sel_idx);
            sig.returns.push(ret_name);
            for (block_idx, ret_map) in sig.block_return_maps.iter_mut().enumerate() {
                let var_name = block_stores
                    .get(block_idx)
                    .and_then(|s| s.get(store_idx))
                    .cloned()
                    .unwrap_or_default();
                ret_map.push(var_name);
            }
        }

        assert_eq!(sig.returns, vec!["ret_0", "ret_1"]);
        assert_eq!(sig.block_return_maps[0], vec!["b", "a"]);
        assert_eq!(sig.block_return_maps[1], vec!["d", "c"]);
    }

    #[test]
    fn add_returns_no_candidates_when_all_stores_already_returned() {
        let sig = make_sig(
            &["arg_0"],
            &["ret_0"],
            &[&["x"], &["y"]],
            &[&["a"], &["b"]],
        );
        let block_stores = vec![
            vec!["a".to_string()],
            vec!["b".to_string()],
        ];

        let existing: std::collections::HashSet<&str> = sig
            .block_return_maps[0]
            .iter()
            .map(|s| s.as_str())
            .collect();
        let ref_stores = &block_stores[0];
        let candidate_indices: Vec<usize> = ref_stores
            .iter()
            .enumerate()
            .filter(|(_, name)| !existing.contains(name.as_str()))
            .map(|(i, _)| i)
            .collect();

        assert!(candidate_indices.is_empty(), "no candidates when all stores are already returns");
    }

    /// End-to-end: plan_extraction provides block_stores, then simulated add_returns
    /// produces valid Python.
    #[test]
    fn simulated_add_returns_produces_valid_python() {
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

        // block_stores should contain the variables stored in each block.
        assert!(!plan.block_stores.is_empty());
        assert!(!plan.block_stores[0].is_empty());

        // Find candidates and add the first one that isn't already returned.
        let existing: std::collections::HashSet<&str> = sig
            .block_return_maps
            .first()
            .map(|m| m.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let ref_stores = &plan.block_stores[0];
        let candidate_indices: Vec<usize> = ref_stores
            .iter()
            .enumerate()
            .filter(|(_, name)| !existing.contains(name.as_str()))
            .map(|(i, _)| i)
            .collect();

        if !candidate_indices.is_empty() {
            // Add first candidate.
            let store_idx = candidate_indices[0];
            let ret_name = format!("ret_{}", sig.returns.len());
            sig.returns.push(ret_name);
            for (block_idx, ret_map) in sig.block_return_maps.iter_mut().enumerate() {
                let var_name = plan.block_stores
                    .get(block_idx)
                    .and_then(|s| s.get(store_idx))
                    .cloned()
                    .unwrap_or_default();
                ret_map.push(var_name);
            }
        }

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );
        assert!(validate_output(&result).is_ok(), "added return must produce valid Python");
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
