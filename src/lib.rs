pub mod diff_extract;
pub mod normalize;
pub mod rewrite;
pub mod scan;
pub mod scope;

use anyhow::{Result, bail};
use ruff_text_size::Ranged;

/// Run the full extract-method pipeline on `source`, targeting `start_line..=end_line`.
///
/// Returns the refactored source code on success.
pub fn extract_method(source: &str, start_line: usize, end_line: usize) -> Result<String> {
    let blocks = scan::find_matches(source, start_line, end_line)?;
    if blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            blocks.len()
        );
    }

    let parsed = ruff_python_parser::parse_module(source)
        .map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;
    let body = &parsed.into_syntax().body;
    let window_size = {
        let target_stmts = normalize::select_stmts(source, body, start_line, end_line);
        target_stmts.len()
    };

    let mut sig_inputs: Vec<(&[ruff_python_ast::Stmt], &[ruff_python_ast::Stmt])> = Vec::new();
    for block in &blocks {
        let idx = body
            .iter()
            .position(|s| s.range().start().to_usize() == block.start_offset)
            .expect("block offset should match a statement");
        let after_start = idx + window_size;
        let block_slice = &body[idx..idx + window_size];
        let after_slice = if after_start < body.len() {
            &body[after_start..]
        } else {
            &[]
        };
        sig_inputs.push((block_slice, after_slice));
    }

    let mut all_divs = Vec::new();
    if sig_inputs.len() >= 2 {
        let (ref_block, _) = &sig_inputs[0];
        for (other_block, _) in sig_inputs.iter().skip(1) {
            let divs = diff_extract::extract_divergences(ref_block, other_block, source, source);
            all_divs.push(divs);
        }
    }

    let sig = scope::unify_signatures(&sig_inputs, &all_divs);
    Ok(rewrite::apply_refactoring(source, &blocks, &sig))
}
