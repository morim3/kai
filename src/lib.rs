pub mod diff_extract;
pub mod interactive;
pub mod normalize;
pub mod rewrite;
pub mod scan;
pub mod scope;

/// Shared test utilities available to all `#[cfg(test)]` modules in this crate.
#[cfg(test)]
pub(crate) mod test_utils {
    use ruff_python_ast::Stmt;

    use crate::scope::FunctionSignature;

    /// Parse Python source and return the top-level body statements.
    pub fn parse_stmts(source: &str) -> Vec<Stmt> {
        ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax()
            .body
    }

    /// Build a `FunctionSignature` from string slices (test convenience).
    /// All `return_param_links` default to `None`.
    pub fn make_sig(
        params: &[&str],
        returns: &[&str],
        arg_maps: &[&[&str]],
        ret_maps: &[&[&str]],
    ) -> FunctionSignature {
        let return_count = returns.len();
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
            return_param_links: vec![None; return_count],
        }
    }
}

use anyhow::{Context, Result, bail};
use ruff_python_ast::ModModule;
use ruff_python_parser::Parsed;
use ruff_text_size::Ranged;

use scan::{MatchedBlock, ScopeContext};
use scope::FunctionSignature;

/// Parse Python source, mapping the parser error to `anyhow`.
pub fn parse_python(source: &str) -> Result<Parsed<ModModule>> {
    ruff_python_parser::parse_module(source).map_err(|e| anyhow::anyhow!("Parse error: {e}"))
}

/// Options for customizing the extract-method refactoring.
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Custom function name (default: `extracted_func_0`).
    pub func_name: Option<String>,
}

/// The result of the planning stage: everything needed to apply the refactoring
/// without holding AST borrows.
#[derive(Debug, Clone)]
pub struct ExtractionPlan {
    /// Unified function signature (params, returns, per-block mappings).
    pub sig: FunctionSignature,
    /// Scope context determining where to place the extracted function.
    pub scope_ctx: ScopeContext,
    /// Pre-collected AST node positions `(byte_offset, byte_len)` for block 0.
    pub ref_node_positions: Vec<(usize, usize)>,
    /// All store (assigned) variables per block, in order of first store.
    /// `block_stores[i]` corresponds to `blocks[i]`. Used by interactive mode
    /// to offer additional return value candidates.
    pub block_stores: Vec<Vec<String>>,
}

/// Stage 1+2: Scan for matches, then compute the extraction plan.
///
/// This function performs parsing, scope analysis, divergence extraction,
/// and signature unification, returning all the owned data needed for
/// `apply_refactoring` without holding any AST borrows.
pub fn plan_extraction(
    source: &str,
    blocks: &[MatchedBlock],
    start_line: usize,
    end_line: usize,
) -> Result<ExtractionPlan> {
    let syntax = parse_python(source)?.into_syntax();
    let top_body = &syntax.body;

    // Determine the scope context: innermost if all blocks share a body,
    // parent if they span sibling scopes.
    let scope_ctx = scan::find_scope_for_matches(top_body, source, start_line, end_line, blocks);

    // Get window size from the innermost body (where the target lives).
    let (inner_body, _) = scan::find_innermost_body(top_body, source, start_line, end_line);
    let window_size = {
        let target_stmts = normalize::select_stmts(source, inner_body, start_line, end_line);
        target_stmts.len()
    };

    // For each block, find its own body and compute after_block from that body.
    let mut sig_inputs: Vec<(&[ruff_python_ast::Stmt], &[ruff_python_ast::Stmt])> = Vec::new();
    for block in blocks {
        let body = scan::find_body_for_block(top_body, source, block.start_offset);
        let idx = body
            .iter()
            .position(|s| s.range().start().to_usize() == block.start_offset)
            .context("block offset does not match any statement")?;
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

    // Pre-collect AST node positions for block 0 so we can drop the AST borrow.
    let ref_stmts = sig_inputs[0].0;
    let ref_node_positions = rewrite::collect_node_positions(ref_stmts);

    // Collect all store variables per block (for interactive return-value addition).
    let block_stores: Vec<Vec<String>> = sig_inputs
        .iter()
        .map(|(block, _)| scope::block_stores(block))
        .collect();

    Ok(ExtractionPlan {
        sig,
        scope_ctx,
        ref_node_positions,
        block_stores,
    })
}

/// Run the full extract-method pipeline on `source`, targeting `start_line..=end_line`.
///
/// Returns the refactored source code on success.
pub fn extract_method(source: &str, start_line: usize, end_line: usize) -> Result<String> {
    extract_method_with_options(source, start_line, end_line, &ExtractOptions::default())
}

/// Run the full extract-method pipeline with custom options.
pub fn extract_method_with_options(
    source: &str,
    start_line: usize,
    end_line: usize,
    options: &ExtractOptions,
) -> Result<String> {
    let blocks = scan::find_matches(source, start_line, end_line)?;
    if blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            blocks.len()
        );
    }

    let plan = plan_extraction(source, &blocks, start_line, end_line)?;
    let func_name = options.func_name.as_deref().unwrap_or("extracted_func_0");
    Ok(rewrite::apply_refactoring(
        source,
        &blocks,
        &plan.ref_node_positions,
        &plan.sig,
        func_name,
        &plan.scope_ctx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_extraction_returns_correct_signature() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let blocks = scan::find_matches(source, 1, 2).unwrap();
        assert_eq!(blocks.len(), 2);

        let plan = plan_extraction(source, &blocks, 1, 2).unwrap();
        // Two variable inputs become params; literals diverge and become params too.
        assert!(!plan.sig.params.is_empty());
        assert!(!plan.ref_node_positions.is_empty());
        assert_eq!(plan.scope_ctx.kind, scan::ScopeKind::Module);
    }

    #[test]
    fn plan_then_apply_matches_one_shot() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let one_shot = extract_method(source, 1, 2).unwrap();

        let blocks = scan::find_matches(source, 1, 2).unwrap();
        let plan = plan_extraction(source, &blocks, 1, 2).unwrap();
        let two_stage = rewrite::apply_refactoring(
            source,
            &blocks,
            &plan.ref_node_positions,
            &plan.sig,
            "extracted_func_0",
            &plan.scope_ctx,
        );

        assert_eq!(one_shot, two_stage, "Two-stage should match one-shot");
    }
}
