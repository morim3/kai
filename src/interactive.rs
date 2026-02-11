use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, MultiSelect};

use crate::rewrite;
use crate::scan::MatchedBlock;
use crate::scope::FunctionSignature;
use crate::{plan_extraction, scan};

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

/// Step 1: Let the user select which matched blocks to include.
///
/// Returns the selected subset of blocks (indices into the original list).
fn select_blocks(source: &str, blocks: &[MatchedBlock]) -> Result<Vec<usize>> {
    if blocks.len() <= 2 {
        // With 2 or fewer blocks, all must be included (need at least 2).
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

/// Step 2: Get the function name from the user.
fn get_function_name(default: &str) -> Result<String> {
    let name: String = Input::new()
        .with_prompt("Function name")
        .default(default.to_string())
        .interact_text()?;
    Ok(name)
}

/// Step 3: Let the user select which parameters to keep.
///
/// Returns indices of selected parameters.
fn select_parameters(sig: &FunctionSignature) -> Result<Vec<usize>> {
    if sig.params.is_empty() {
        return Ok(vec![]);
    }

    eprintln!("\nParameters (per-block values):");
    let items: Vec<String> = sig
        .params
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let values: Vec<&str> = sig
                .block_arg_maps
                .iter()
                .map(|m| m.get(i).map(|s| s.as_str()).unwrap_or("?"))
                .collect();
            format!("{name}: {}", values.join(" | "))
        })
        .collect();

    for item in &items {
        eprintln!("  [x] {item}");
    }

    let defaults: Vec<bool> = vec![true; sig.params.len()];
    let selections = MultiSelect::new()
        .with_prompt("Select parameters to keep [Space=toggle, Enter=confirm]")
        .items(&items)
        .defaults(&defaults)
        .interact()?;

    Ok(selections)
}

/// Step 4: Let the user rename parameters.
fn rename_parameters(sig: &mut FunctionSignature, kept_indices: &[usize]) -> Result<()> {
    for &i in kept_indices {
        let current = &sig.params[i];
        let new_name: String = Input::new()
            .with_prompt(format!("Rename {current}"))
            .default(current.clone())
            .interact_text()?;
        if new_name != *current {
            sig.params[i] = new_name;
        }
    }
    Ok(())
}

/// Step 5a: Let the user select which return values to keep.
///
/// Returns indices of selected returns.
fn select_returns(sig: &FunctionSignature) -> Result<Vec<usize>> {
    if sig.returns.is_empty() {
        return Ok(vec![]);
    }

    eprintln!("\nReturns (per-block names):");
    let items: Vec<String> = sig
        .returns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let values: Vec<&str> = sig
                .block_return_maps
                .iter()
                .map(|m| m.get(i).map(|s| s.as_str()).unwrap_or("?"))
                .collect();
            format!("{name}: {}", values.join(" | "))
        })
        .collect();

    for item in &items {
        eprintln!("  [x] {item}");
    }

    let defaults: Vec<bool> = vec![true; sig.returns.len()];
    let selections = MultiSelect::new()
        .with_prompt("Select return values to keep [Space=toggle, Enter=confirm]")
        .items(&items)
        .defaults(&defaults)
        .interact()?;

    Ok(selections)
}

/// Step 5b: Let the user rename return values.
fn rename_returns(sig: &mut FunctionSignature, kept_indices: &[usize]) -> Result<()> {
    if sig.returns.is_empty() {
        return Ok(());
    }

    for &i in kept_indices {
        let current = sig.returns[i].clone();
        let new_name: String = Input::new()
            .with_prompt(format!("Rename {current}"))
            .default(current.clone())
            .interact_text()?;
        if new_name != current {
            sig.returns[i] = new_name;
        }
    }
    Ok(())
}

/// Remove parameters at indices NOT in `kept` from the signature.
///
/// Updates `sig.params` and each entry in `sig.block_arg_maps`.
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
///
/// Updates `sig.returns` and each entry in `sig.block_return_maps`.
pub fn remove_returns(sig: &mut FunctionSignature, kept: &[usize]) {
    sig.returns = kept.iter().map(|&i| sig.returns[i].clone()).collect();
    for ret_map in &mut sig.block_return_maps {
        *ret_map = kept
            .iter()
            .filter_map(|&i| ret_map.get(i).cloned())
            .collect();
    }
}

/// Run the interactive extraction workflow.
///
/// Guides the user through block selection, function naming,
/// parameter customization, and result preview.
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

    // Step 3: Parameter selection.
    let kept_params = select_parameters(&sig)?;

    // Step 4: Parameter rename (only kept params).
    rename_parameters(&mut sig, &kept_params)?;

    // Apply parameter removal.
    if kept_params.len() < sig.params.len() {
        remove_params(&mut sig, &kept_params);
    }

    // Step 5a: Return value selection.
    let kept_returns = select_returns(&sig)?;

    // Step 5b: Return value rename (only kept returns).
    rename_returns(&mut sig, &kept_returns)?;

    // Apply return removal.
    if kept_returns.len() < sig.returns.len() {
        remove_returns(&mut sig, &kept_returns);
    }

    // Stage 3: Apply refactoring.
    let result = rewrite::apply_refactoring(
        source,
        &blocks,
        &plan.ref_node_positions,
        &sig,
        &func_name,
        &plan.scope_ctx,
    );

    // Step 6: Preview and confirm.
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

    /// Simulate the full interactive pipeline programmatically:
    /// scan → select all blocks → plan → keep all params → apply.
    /// Result must match non-interactive mode.
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
        // Non-interactive result.
        let expected = crate::extract_method(source, 1, 2).unwrap();

        // Simulate interactive: scan, select all, plan, apply with no modifications.
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

    /// Simulate interactive with block subset selection (drop block 3).
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
        assert_eq!(all_blocks.len(), 3);

        // Select only blocks 0 and 1 (drop block 2).
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

        // Block 3 (e = 100, f = e + 200) should remain untouched.
        assert!(result.contains("e = 100"));
        assert!(result.contains("f = e + 200"));
        // Blocks 1 and 2 should be replaced with calls.
        assert!(result.contains("extracted_func_0(1, 2)"));
        assert!(result.contains("extracted_func_0(10, 20)"));
        // The function def should exist.
        assert!(result.contains("def extracted_func_0(arg_0, arg_1):"));
    }

    /// Simulate interactive with parameter removal.
    #[test]
    fn simulated_interactive_param_removal() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        // sig.params should be ["arg_0", "arg_1"] (variable + literal divergence).
        assert_eq!(sig.params.len(), 2);

        // Remove param at index 1 (the literal divergence).
        remove_params(&mut sig, &[0]);
        assert_eq!(sig.params, vec!["arg_0"]);
        assert_eq!(sig.block_arg_maps[0].len(), 1);
        assert_eq!(sig.block_arg_maps[1].len(), 1);

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "my_func",
            &plan.scope_ctx,
        );
        assert!(result.contains("def my_func(arg_0):"));
        assert!(result.contains("my_func(1)"));
        assert!(result.contains("my_func(10)"));
    }

    /// Simulate interactive with parameter rename.
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

        // Rename arg_0 → "value", arg_1 → "offset"
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
        // The body should use the renamed params.
        assert!(result.contains("value"));
        assert!(result.contains("offset"));
    }

    /// Simulate interactive with return value rename.
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
        assert_eq!(blocks.len(), 2);

        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        // Rename return if present.
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
        // The function should have a return statement.
        if !sig.returns.is_empty() {
            assert!(result.contains("return output"));
        }
    }

    /// Simulate interactive with return value removal.
    #[test]
    fn simulated_interactive_return_removal() {
        let source = "\
a = 1
b = a + 2
print(b)
c = 10
d = c + 20
print(d)
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        assert_eq!(blocks.len(), 2);

        let plan = crate::plan_extraction(source, &blocks, 1, 2).unwrap();
        let mut sig = plan.sig.clone();

        // Remove all returns.
        assert!(!sig.returns.is_empty(), "test expects returns to exist");
        remove_returns(&mut sig, &[]);

        let result = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );
        // No return statement in function body.
        assert!(!result.contains("return "));
        // Calls should have no assignment target.
        assert!(result.contains("\nextracted_func_0(") || result.contains("extracted_func_0(1"));
        assert!(!result.contains("b = extracted_func_0("));
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
