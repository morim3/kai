use std::collections::HashMap;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, MultiSelect};

use crate::rewrite;
use crate::scan::MatchedBlock;
use crate::scope::FunctionSignature;
use crate::{SourcedBlock, plan_extraction, plan_extraction_multi, scan};

// ── Validation ───────────────────────────────────────────────────────

/// Python keywords that cannot be used as identifiers.
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
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

/// Validate the rename map built from `sig` is well-defined and injective.
///
/// Checks two invariants:
/// 1. No original name maps to two different new names (HashMap conflict).
/// 2. No two different original names map to the same new name (variable merge).
pub fn validate_rename_map(sig: &FunctionSignature) -> Result<()> {
    let map = sig.rename_map();

    // Check well-definedness: no original maps to two different new names.
    // sig.rename_map() lets returns override params (HashMap last-write-wins),
    // so we must explicitly check for conflicts.
    if let (Some(arg_map), Some(ret_map)) =
        (sig.block_arg_maps.first(), sig.block_return_maps.first())
    {
        let param_map: HashMap<&str, &str> = arg_map
            .iter()
            .enumerate()
            .filter(|(_, o)| is_valid_python_ident(o))
            .map(|(i, o)| (o.as_str(), sig.params[i].as_str()))
            .collect();
        for (i, original) in ret_map.iter().enumerate() {
            if let Some(&param_name) = param_map.get(original.as_str())
                && param_name != sig.returns[i]
            {
                bail!(
                    "Variable '{}' maps to both '{}' (parameter) and '{}' (return) \
                     — they must have the same name because the variable is both \
                     read and written in the block",
                    original,
                    param_name,
                    sig.returns[i]
                );
            }
        }
    }

    // Check injectivity: different originals → different new names.
    let mut reverse: HashMap<&str, &str> = HashMap::new();
    for (&original, &new_name) in &map {
        if !is_valid_python_ident(original) {
            continue; // literal values don't participate in rename
        }
        if let Some(&other_original) = reverse.get(new_name)
            && other_original != original
        {
            bail!(
                "Variables '{}' and '{}' both renamed to '{}' \
                 — this would merge two different variables into one",
                other_original,
                original,
                new_name
            );
        }
        reverse.insert(new_name, original);
    }

    // Check duplicate returns (e.g. `return a, a`).
    for (i, ret_a) in sig.returns.iter().enumerate() {
        for ret_b in sig.returns.iter().skip(i + 1) {
            if ret_a == ret_b {
                bail!(
                    "Duplicate return name '{}' — each return value must have a unique name",
                    ret_a
                );
            }
        }
    }

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
        let truncate_at = preview
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i >= max_len)
            .unwrap_or(preview.len());
        format!("{}...", &preview[..truncate_at])
    } else {
        preview
    }
}

/// Find indices into `block_stores[0]` for variables that are not already returned.
///
/// Returns indices into the reference block's store list, suitable for offering
/// as additional return value candidates.
pub fn return_candidates(sig: &FunctionSignature, block_stores: &[Vec<String>]) -> Vec<usize> {
    let existing: std::collections::HashSet<&str> = sig
        .block_return_maps
        .first()
        .map(|m| m.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let ref_stores = match block_stores.first() {
        Some(s) => s,
        None => return Vec::new(),
    };

    ref_stores
        .iter()
        .enumerate()
        .filter(|(_, name)| !existing.contains(name.as_str()))
        .map(|(i, _)| i)
        .collect()
}

// ── Interactive steps ────────────────────────────────────────────────

/// Shared interactive block selection: display items, let user toggle, require ≥2.
fn prompt_block_selection(items: &[String]) -> Result<Vec<usize>> {
    if items.len() <= 2 {
        return Ok((0..items.len()).collect());
    }

    eprintln!("\nFound {} matching blocks:", items.len());
    for item in items {
        eprintln!("  {item}");
    }

    let defaults: Vec<bool> = vec![true; items.len()];
    let selections = MultiSelect::new()
        .with_prompt("Select blocks to extract [Space=toggle, Enter=confirm]")
        .items(items)
        .defaults(&defaults)
        .interact()?;

    if selections.len() < 2 {
        bail!("Need at least 2 blocks selected for extraction.");
    }

    Ok(selections)
}

/// Step 1: Let the user select which matched blocks to include.
fn select_blocks(source: &str, blocks: &[MatchedBlock]) -> Result<Vec<usize>> {
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
    prompt_block_selection(&items)
}

/// Step 1 (multi-file): Let the user select which matched blocks to include.
fn select_sourced_blocks(
    sources: &[&str],
    file_names: &[&str],
    blocks: &[SourcedBlock],
) -> Result<Vec<usize>> {
    let items: Vec<String> = blocks
        .iter()
        .enumerate()
        .map(|(i, sb)| {
            let source = sources[sb.source_index];
            let preview = block_preview(source, &sb.block, 50);
            format!(
                "[{}] {} lines {}-{}: {}",
                i + 1,
                file_names[sb.source_index],
                sb.block.start_line,
                sb.block.end_line,
                preview
            )
        })
        .collect();
    prompt_block_selection(&items)
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
/// each item with validation and within-collection duplicate checking.
fn rename_collection(
    names: &mut [String],
    per_block_maps: &[Vec<String>],
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

            let dup = names
                .iter()
                .enumerate()
                .any(|(j, n)| j != i && n == &new_name);
            if dup {
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

/// Auto-sync output=input returns: if a return's original variable name
/// matches a param's original variable name, update the return to use the
/// (possibly renamed) param name.
pub fn sync_linked_returns(sig: &mut FunctionSignature) {
    if let (Some(arg_map), Some(ret_map)) =
        (sig.block_arg_maps.first(), sig.block_return_maps.first())
    {
        for (ret_idx, ret_orig) in ret_map.iter().enumerate() {
            if let Some(param_idx) = arg_map.iter().position(|a| a == ret_orig) {
                sig.returns[ret_idx] = sig.params[param_idx].clone();
            }
        }
    }
}

/// Step 3: Rename parameters with validation.
///
/// After renaming, returns whose original variable matches a param's original
/// variable (output=input) are synced to the new param name.
fn rename_parameters(sig: &mut FunctionSignature) -> Result<()> {
    rename_collection(
        &mut sig.params,
        &sig.block_arg_maps,
        "Parameters (per-block values)",
        "parameter",
    )?;
    sync_linked_returns(sig);
    Ok(())
}

/// Interactive naming flow shared by single-file and multi-file paths.
/// Returns the chosen function name.
fn interactive_naming(sig: &mut FunctionSignature, block_stores: &[Vec<String>]) -> Result<String> {
    let func_name = get_function_name("extracted_func_0")?;
    rename_parameters(sig)?;
    rename_returns(sig)?;
    add_returns(sig, block_stores)?;
    validate_rename_map(sig)?;
    Ok(func_name)
}

/// Step 4: Rename return values with validation.
fn rename_returns(sig: &mut FunctionSignature) -> Result<()> {
    rename_collection(
        &mut sig.returns,
        &sig.block_return_maps,
        "Returns (per-block names)",
        "return value",
    )
}

/// Step 5: Offer additional return value candidates from block stores.
///
/// Shows variables stored in the block that are NOT already returns.
/// User can select additional ones to include as return values.
fn add_returns(sig: &mut FunctionSignature, block_stores: &[Vec<String>]) -> Result<()> {
    let candidate_indices = return_candidates(sig, block_stores);
    if candidate_indices.is_empty() {
        return Ok(());
    }

    let ref_stores = &block_stores[0];
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
        let ret_name = crate::scope::default_return_name(ret_count + sel_idx);
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

    // Rename newly added returns.
    let new_returns = &mut sig.returns[ret_count..];
    let new_maps: Vec<Vec<String>> = sig
        .block_return_maps
        .iter()
        .map(|m| m[ret_count..].to_vec())
        .collect();
    rename_collection(
        new_returns,
        &new_maps,
        "Rename added returns",
        "return value",
    )?;

    Ok(())
}

// ── Main entry point ─────────────────────────────────────────────────

/// Run the interactive extraction workflow.
///
/// Flow: block selection → function name → param rename → return rename
///       → add returns → final validation → preview.
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

    // Steps 2-5: Function name, parameter/return rename, add returns.
    let func_name = interactive_naming(&mut sig, &plan.block_stores)?;

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

    // Preview and confirm.
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

/// Run the interactive extraction workflow for multiple files.
///
/// Same interactive flow as single-file, but scans across files
/// and applies refactoring to all of them.
pub fn run_interactive_multi(
    sources: &[&str],
    file_paths: &[&str],
    start_line: usize,
    end_line: usize,
    write: bool,
    show_diff: bool,
    target_file_stem: &str,
) -> Result<()> {
    // Stage 1: Scan target + extra files.
    let all_blocks = crate::scan_all_sources(sources, start_line, end_line)?;

    // Step 1: Block selection.
    let selected_indices = select_sourced_blocks(sources, file_paths, &all_blocks)?;
    let blocks: Vec<SourcedBlock> = selected_indices
        .iter()
        .map(|&i| all_blocks[i].clone())
        .collect();

    // Stage 2: Compute extraction plan.
    let plan = plan_extraction_multi(sources, &blocks, start_line, end_line)?;
    let mut sig = plan.sig;

    // Steps 2-5: Function name, parameter/return rename, add returns.
    let func_name = interactive_naming(&mut sig, &plan.block_stores)?;

    // Stage 3: Apply refactoring.
    let results = rewrite::apply_refactoring_multi(
        sources,
        &blocks,
        &plan.ref_node_positions,
        &sig,
        &func_name,
        &plan.scope_ctx,
        target_file_stem,
    );

    // Validate all outputs.
    for (i, result) in results.iter().enumerate() {
        if result != sources[i] {
            validate_output(result)?;
        }
    }

    // Preview.
    if show_diff {
        eprintln!("\n--- Preview (diff) ---");
        for (i, result) in results.iter().enumerate() {
            if result != sources[i] {
                let diff = rewrite::unified_diff(sources[i], result, file_paths[i]);
                print!("{diff}");
            }
        }
    } else {
        eprintln!("\n--- Preview ---");
        for (i, result) in results.iter().enumerate() {
            if result != sources[i] {
                println!("=== {} ===", file_paths[i]);
                print!("{result}");
            }
        }
    }

    // Write.
    if write {
        let confirm = Confirm::new()
            .with_prompt("Write to all modified files?")
            .default(false)
            .interact()?;
        if confirm {
            for (i, result) in results.iter().enumerate() {
                if result != sources[i] {
                    std::fs::write(file_paths[i], result)?;
                    eprintln!("Wrote refactored code to {}", file_paths[i]);
                }
            }
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
        assert!(!is_valid_python_ident(""));
        assert!(!is_valid_python_ident("123x"));
        assert!(!is_valid_python_ident("my-var"));
        assert!(!is_valid_python_ident("my var"));
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

    // ── validate_rename_map tests ──

    #[test]
    fn rename_map_ok_for_independent_params_and_returns() {
        let sig = make_sig(
            &["x", "y"],
            &["result"],
            &[&["a", "b"], &["c", "d"]],
            &[&["r"], &["s"]],
        );
        assert!(validate_rename_map(&sig).is_ok());
    }

    #[test]
    fn rename_map_ok_when_output_equals_input_same_name() {
        // a → x for both param and return: consistent mapping.
        let sig = make_sig(&["x"], &["x"], &[&["a"], &["b"]], &[&["a"], &["b"]]);
        assert!(validate_rename_map(&sig).is_ok());
    }

    #[test]
    fn rename_map_rejects_conflicting_param_return() {
        // a → x (param) but a → y (return): conflict.
        let sig = make_sig(&["x"], &["y"], &[&["a"], &["b"]], &[&["a"], &["b"]]);
        let err = validate_rename_map(&sig).unwrap_err();
        assert!(err.to_string().contains("maps to both"), "{err}");
    }

    #[test]
    fn rename_map_rejects_variable_merge() {
        // a → z and b → z: two originals merge into one.
        let sig = make_sig(&["z"], &["z"], &[&["a"], &["c"]], &[&["b"], &["d"]]);
        let err = validate_rename_map(&sig).unwrap_err();
        assert!(err.to_string().contains("merge"), "{err}");
    }

    #[test]
    fn rename_map_rejects_duplicate_returns() {
        // Different originals (r1, r2) mapped to same return name "a" → merge error.
        let sig = make_sig(
            &["x"],
            &["a", "a"],
            &[&["v"], &["w"]],
            &[&["r1", "r2"], &["s1", "s2"]],
        );
        assert!(validate_rename_map(&sig).is_err());

        // Same original mapped to same return name (self-duplicate) → duplicate error.
        let sig = make_sig(
            &["x"],
            &["a", "a"],
            &[&["v"], &["w"]],
            &[&["r1", "r1"], &["s1", "s1"]],
        );
        let err = validate_rename_map(&sig).unwrap_err();
        assert!(err.to_string().contains("Duplicate return"), "{err}");
    }

    #[test]
    fn rename_map_skips_literals() {
        // Literal "1" in arg_map should not be treated as a variable rename.
        let sig = make_sig(
            &["x", "lit"],
            &[],
            &[&["a", "1"], &["b", "10"]],
            &[&[], &[]],
        );
        assert!(validate_rename_map(&sig).is_ok());
    }

    // ── Simulated pipeline tests ──

    /// Helper: plan extraction, optionally modify sig, apply, validate.
    fn plan_apply(
        source: &str,
        blocks: &[MatchedBlock],
        start: usize,
        end: usize,
        func_name: &str,
        modify_sig: impl FnOnce(&mut FunctionSignature),
    ) -> String {
        let plan = crate::plan_extraction(source, blocks, start, end).unwrap();
        let mut sig = plan.sig.clone();
        modify_sig(&mut sig);
        assert!(validate_rename_map(&sig).is_ok());
        let result = rewrite::apply_refactoring(
            source,
            blocks,
            &plan.ref_node_positions,
            &sig,
            func_name,
            &plan.scope_ctx,
        );
        assert!(validate_output(&result).is_ok(), "output must be valid Python");
        result
    }

    #[test]
    fn simulated_interactive_matches_non_interactive() {
        let source = "a = 1\nb = a + 2\nc = 10\nd = c + 20\ne = 100\nf = e + 200\n";
        let expected = crate::extract_method(source, 1, 2).unwrap();
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let result = plan_apply(source, &blocks, 1, 2, "extracted_func_0", |_| {});
        assert_eq!(result, expected);
    }

    #[test]
    fn simulated_interactive_block_subset() {
        let source = "a = 1\nb = a + 2\nc = 10\nd = c + 20\ne = 100\nf = e + 200\n";
        let all_blocks = scan::find_matches(source, 1, 2).unwrap();
        let blocks: Vec<MatchedBlock> = vec![all_blocks[0].clone(), all_blocks[1].clone()];
        let result = plan_apply(source, &blocks, 1, 2, "extracted_func_0", |_| {});
        assert!(result.contains("e = 100"));
        assert!(result.contains("f = e + 200"));
        assert!(result.contains("extracted_func_0(1, 2)"));
        assert!(result.contains("extracted_func_0(10, 20)"));
    }

    #[test]
    fn simulated_interactive_param_rename() {
        let source = "a = 1\nb = a + 2\nc = 10\nd = c + 20\n";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let result = plan_apply(source, &blocks, 1, 2, "compute", |sig| {
            sig.params[0] = "value".into();
            sig.params[1] = "offset".into();
        });
        assert!(result.contains("def compute(value, offset):"));
    }

    #[test]
    fn simulated_interactive_return_rename() {
        let source = "a = 1\nb = a + 2\nprint(b)\nc = 10\nd = c + 20\nprint(d)\n";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let result = plan_apply(source, &blocks, 1, 2, "extracted_func_0", |sig| {
            if !sig.returns.is_empty() {
                sig.returns[0] = "output".into();
            }
        });
        assert!(result.contains("return output"));
    }

    #[test]
    fn generated_output_valid_with_custom_names() {
        let source = "a = 1\nb = a + 2\nprint(b)\nc = 10\nd = c + 20\nprint(d)\n";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        plan_apply(source, &blocks, 1, 2, "my_func", |sig| {
            sig.params[0] = "x".into();
            if !sig.returns.is_empty() {
                sig.returns[0] = "result".into();
            }
        });
    }

    // ── add_returns logic tests (unit-level, no TTY) ──

    /// Helper: apply add-returns logic without TTY (mirrors the core of `add_returns()`).
    fn apply_additional_returns(
        sig: &mut FunctionSignature,
        block_stores: &[Vec<String>],
        selected_candidate_indices: &[usize],
    ) {
        let candidate_indices = return_candidates(sig, block_stores);
        let ret_count = sig.returns.len();
        for (sel_idx, &sel) in selected_candidate_indices.iter().enumerate() {
            let store_idx = candidate_indices[sel];
            let ret_name = crate::scope::default_return_name(ret_count + sel_idx);
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
    }

    #[test]
    fn add_returns_logic_adds_store_variables() {
        let mut sig = make_sig(&["arg_0"], &["ret_0"], &[&["x"], &["y"]], &[&["b"], &["d"]]);
        let block_stores = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string()],
        ];

        let candidates = return_candidates(&sig, &block_stores);
        assert_eq!(candidates, vec![0]);

        apply_additional_returns(&mut sig, &block_stores, &[0]);
        assert_eq!(sig.returns, vec!["ret_0", "ret_1"]);
        assert_eq!(sig.block_return_maps[0], vec!["b", "a"]);
        assert_eq!(sig.block_return_maps[1], vec!["d", "c"]);
    }

    #[test]
    fn add_returns_no_candidates_when_all_stores_already_returned() {
        let sig = make_sig(&["arg_0"], &["ret_0"], &[&["x"], &["y"]], &[&["a"], &["b"]]);
        let block_stores = vec![vec!["a".to_string()], vec!["b".to_string()]];
        assert!(return_candidates(&sig, &block_stores).is_empty());
    }

    #[test]
    fn simulated_add_returns_produces_valid_python() {
        let source = "a = 1\nb = a + 2\nprint(b)\nc = 10\nd = c + 20\nprint(d)\n";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        let candidates = return_candidates(&sig, &plan.block_stores);
        if !candidates.is_empty() {
            apply_additional_returns(&mut sig, &plan.block_stores, &[0]);
        }

        assert!(validate_rename_map(&sig).is_ok());
        let result = plan_apply(source, &blocks, 1, 2, "extracted_func_0", |s| {
            *s = sig.clone();
        });
        assert!(validate_output(&result).is_ok());
    }

    // ── auto-sync test ──

    #[test]
    fn auto_sync_linked_return_on_param_rename() {
        let source = "a += 1\nprint(a)\nb += 1\nprint(b)\n";
        let blocks = scan::find_matches(source, 1, 1).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 1).unwrap();
        let mut sig = plan.sig.clone();

        assert_eq!(sig.returns[0], "arg_0", "unify_signatures links output=input");

        // Simulate param rename and auto-sync using the extracted helper.
        sig.params[0] = "x".into();
        sync_linked_returns(&mut sig);

        assert_eq!(sig.returns[0], "x", "return must follow param rename");

        let result = plan_apply(source, &blocks, 1, 1, "inc", |s| *s = sig.clone());
        assert!(result.contains("def inc(x):"));
        assert!(result.contains("return x"));
        assert!(!result.contains("arg_0"));
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

    #[test]
    fn block_preview_multibyte_utf8() {
        let source = "x = \"日本語のテスト文字列\" + \"もっと長い文字列\"\n";
        let block = MatchedBlock {
            start_line: 1,
            end_line: 1,
            start_offset: 0,
            end_offset: source.len() - 1,
        };
        // Should not panic on multi-byte truncation
        let preview = block_preview(source, &block, 10);
        assert!(preview.ends_with("..."));
    }
}
