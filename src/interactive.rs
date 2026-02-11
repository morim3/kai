use std::collections::HashMap;

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

/// Validate the rename map built from `sig` is well-defined and injective.
///
/// Checks two invariants:
/// 1. No original name maps to two different new names (HashMap conflict).
/// 2. No two different original names map to the same new name (variable merge).
pub fn validate_rename_map(sig: &FunctionSignature) -> Result<()> {
    let mut map: HashMap<&str, &str> = HashMap::new();

    // Params: original variable name → param name.
    if let Some(arg_map) = sig.block_arg_maps.first() {
        for (i, original) in arg_map.iter().enumerate() {
            if !is_valid_python_ident(original) {
                continue; // literal value, not a variable rename
            }
            map.insert(original, &sig.params[i]);
        }
    }

    // Returns: original variable name → return name.
    if let Some(ret_map) = sig.block_return_maps.first() {
        for (i, original) in ret_map.iter().enumerate() {
            if let Some(&existing) = map.get(original.as_str())
                && existing != sig.returns[i] {
                    bail!(
                        "Variable '{}' maps to both '{}' (parameter) and '{}' (return) \
                         — they must have the same name because the variable is both \
                         read and written in the block",
                        original,
                        existing,
                        sig.returns[i]
                    );
                }
            map.insert(original, &sig.returns[i]);
        }
    }

    // Check injectivity: different originals → different new names.
    let mut reverse: HashMap<&str, &str> = HashMap::new();
    for (&original, &new_name) in &map {
        if let Some(&other_original) = reverse.get(new_name)
            && other_original != original {
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

    // Auto-sync output=input returns: if a return's original variable name
    // matches a param's original variable name, they are the same variable.
    if let (Some(arg_map), Some(ret_map)) =
        (sig.block_arg_maps.first(), sig.block_return_maps.first())
    {
        for (ret_idx, ret_orig) in ret_map.iter().enumerate() {
            if let Some(param_idx) = arg_map.iter().position(|a| a == ret_orig) {
                sig.returns[ret_idx] = sig.params[param_idx].clone();
            }
        }
    }
    Ok(())
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

    // Rename newly added returns.
    let new_returns = &mut sig.returns[ret_count..];
    let new_maps: Vec<Vec<String>> = sig
        .block_return_maps
        .iter()
        .map(|m| m[ret_count..].to_vec())
        .collect();
    rename_collection(new_returns, &new_maps, "Rename added returns", "return value")?;

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

    // Step 2: Function name.
    let func_name = get_function_name("extracted_func_0")?;

    // Step 3: Parameter rename.
    rename_parameters(&mut sig)?;

    // Step 4: Return value rename.
    rename_returns(&mut sig)?;

    // Step 5: Add additional return values.
    add_returns(&mut sig, &plan.block_stores)?;

    // Final validation: check rename map consistency before generating code.
    validate_rename_map(&sig)?;

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
        let sig = make_sig(
            &["x"],
            &["x"],
            &[&["a"], &["b"]],
            &[&["a"], &["b"]],
        );
        assert!(validate_rename_map(&sig).is_ok());
    }

    #[test]
    fn rename_map_rejects_conflicting_param_return() {
        // a → x (param) but a → y (return): conflict.
        let sig = make_sig(
            &["x"],
            &["y"],
            &[&["a"], &["b"]],
            &[&["a"], &["b"]],
        );
        let err = validate_rename_map(&sig).unwrap_err();
        assert!(err.to_string().contains("maps to both"), "{err}");
    }

    #[test]
    fn rename_map_rejects_variable_merge() {
        // a → z and b → z: two originals merge into one.
        let sig = make_sig(
            &["z"],
            &["z"],
            &[&["a"], &["c"]],
            &[&["b"], &["d"]],
        );
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

        assert!(validate_rename_map(&sig).is_ok());
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

        assert!(validate_rename_map(&sig).is_ok());
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
        assert!(validate_rename_map(&sig).is_ok());
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

    #[test]
    fn add_returns_logic_adds_store_variables() {
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

        assert_eq!(candidate_indices, vec![0]);

        let ret_count = sig.returns.len();
        for (sel_idx, &sel) in [0usize].iter().enumerate() {
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
        let block_stores = vec![vec!["a".to_string()], vec!["b".to_string()]];

        let existing: std::collections::HashSet<&str> = sig.block_return_maps[0]
            .iter()
            .map(|s| s.as_str())
            .collect();
        let candidates: Vec<_> = block_stores[0]
            .iter()
            .filter(|name| !existing.contains(name.as_str()))
            .collect();

        assert!(candidates.is_empty());
    }

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

        assert!(!plan.block_stores.is_empty());
        assert!(!plan.block_stores[0].is_empty());

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
            let store_idx = candidate_indices[0];
            let ret_name = format!("ret_{}", sig.returns.len());
            sig.returns.push(ret_name);
            for (block_idx, ret_map) in sig.block_return_maps.iter_mut().enumerate() {
                let var_name = plan
                    .block_stores
                    .get(block_idx)
                    .and_then(|s| s.get(store_idx))
                    .cloned()
                    .unwrap_or_default();
                ret_map.push(var_name);
            }
        }

        assert!(validate_rename_map(&sig).is_ok());
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

    // ── auto-sync test ──

    /// When output=input (e.g. `a += 1`), rename_parameters auto-syncs the
    /// return (which was set to `arg_N` by unify_signatures).
    #[test]
    fn auto_sync_linked_return_on_param_rename() {
        let source = "\
a += 1
print(a)
b += 1
print(b)
";
        let blocks = scan::find_matches(source, 1, 1).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 1).unwrap();
        let mut sig = plan.sig.clone();

        assert_eq!(sig.returns[0], "arg_0", "unify_signatures links output=input");

        // Simulate param rename: arg_0 → x.
        sig.params[0] = "x".to_string();
        // Apply the same auto-sync logic as rename_parameters.
        for ret in &mut sig.returns {
            if let Some(idx) = ret.strip_prefix("arg_").and_then(|n| n.parse::<usize>().ok()) {
                if idx < sig.params.len() {
                    *ret = sig.params[idx].clone();
                }
            }
        }

        assert_eq!(sig.returns[0], "x", "return must follow param rename");
        assert!(validate_rename_map(&sig).is_ok());

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "inc",
            &plan.scope_ctx,
        );
        assert!(result.contains("def inc(x):"));
        assert!(result.contains("return x"));
        assert!(!result.contains("arg_0"));
        assert!(validate_output(&result).is_ok());
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
