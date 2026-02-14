pub mod diff_extract;
pub mod interactive;
pub mod normalize;
pub mod rewrite;
pub mod safety;
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
    pub fn make_sig(
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
}

use anyhow::{Context, Result, bail};
use ruff_python_ast::{ModModule, Stmt};
use ruff_python_parser::Parsed;
use ruff_text_size::Ranged;

use diff_extract::Divergence;
use scan::{MatchedBlock, ScopeContext, ScopeKind};
use scope::FunctionSignature;

/// Per-block statement slices and their after-block statements.
type BlockContexts<'a> = (Vec<&'a [Stmt]>, Vec<Vec<&'a Stmt>>);

/// Divergences, per-comparison literal offsets, and the union of divergent literal offsets.
type DivergenceResult = (Vec<Vec<Divergence>>, Vec<Vec<usize>>, Vec<usize>);

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
    /// Pre-collected AST node positions for block 0.
    pub ref_node_positions: Vec<rewrite::NodePosition>,
    /// All store (assigned) variables per block, in order of first store.
    /// `block_stores[i]` corresponds to `blocks[i]`. Used by interactive mode
    /// to offer additional return value candidates.
    pub block_stores: Vec<Vec<String>>,
    /// Byte offsets (in block 0's source) of divergent literal nodes.
    /// Used to avoid replacing non-divergent literals that happen to share
    /// the same text as a divergent one.
    pub divergent_literal_offsets: Vec<usize>,
}

/// A matched block tagged with the index of the source file it came from.
#[derive(Debug, Clone)]
pub struct SourcedBlock {
    pub block: MatchedBlock,
    /// 0 = target file, 1+ = additional files.
    pub source_index: usize,
}

/// Stage 0: Scan target file and optional extra files for matching blocks.
///
/// Returns all matching blocks tagged with their source file index.
/// `sources[0]` is the target file; remaining entries are extra files to search.
pub fn scan_all_sources(
    sources: &[&str],
    start_line: usize,
    end_line: usize,
) -> Result<Vec<SourcedBlock>> {
    let (target_hash, window_size, target_matches) =
        scan::find_matches_with_hash(sources[0], start_line, end_line)?;

    let mut all_blocks: Vec<SourcedBlock> = target_matches
        .into_iter()
        .map(|b| SourcedBlock {
            block: b,
            source_index: 0,
        })
        .collect();

    for (i, src) in sources.iter().enumerate().skip(1) {
        let extra = scan::find_matches_in_file(src, target_hash, window_size);
        all_blocks.extend(extra.into_iter().map(|b| SourcedBlock {
            block: b,
            source_index: i,
        }));
    }

    if all_blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            all_blocks.len()
        );
    }

    Ok(all_blocks)
}

/// Determine the scope context for function placement.
///
/// Cross-file refactoring forces module-level placement (to make the function importable).
/// Single-file uses the LCA scope of all matched blocks.
fn resolve_scope_context(
    target_body: &[Stmt],
    target_source: &str,
    blocks: &[SourcedBlock],
    is_cross_file: bool,
) -> ScopeContext {
    if is_cross_file {
        ScopeContext {
            kind: ScopeKind::Module,
            body_start_offset: 0,
            indent: String::new(),
            class_def_offset: None,
            parent_indent: None,
        }
    } else {
        let target_blocks: Vec<MatchedBlock> = blocks
            .iter()
            .filter(|b| b.source_index == 0)
            .map(|b| b.block.clone())
            .collect();
        scan::find_scope_for_matches(target_body, target_source, &target_blocks)
    }
}

/// For each block, locate its statement slice and collect after-block statements
/// up to the enclosing scope boundary.
///
/// Returns `(block_slices, after_stmts_per_block)` where each entry corresponds
/// to one element of `blocks`.
fn collect_block_contexts<'a>(
    sources: &[&str],
    parsed: &'a [ModModule],
    blocks: &[SourcedBlock],
    window_size: usize,
) -> Result<BlockContexts<'a>> {
    let mut block_slices: Vec<&[Stmt]> = Vec::new();
    let mut after_stmts_per_block: Vec<Vec<&Stmt>> = Vec::new();

    for sourced in blocks {
        let src = sources[sourced.source_index];
        let body_stmts = &parsed[sourced.source_index].body;
        let block_offset = sourced.block.start_offset;
        let block_line = normalize::line_of_offset(src, block_offset);
        let info = scan::find_scopes(body_stmts, src, block_line, block_line);

        let idx = info
            .inner_body
            .iter()
            .position(|s| s.range().start().to_usize() == block_offset)
            .context("block offset does not match any statement")?;
        block_slices.push(&info.inner_body[idx..idx + window_size]);

        let after_stmts = scan::collect_after_stmts(info.scope_body, block_offset, window_size);
        after_stmts_per_block.push(after_stmts);
    }

    Ok((block_slices, after_stmts_per_block))
}

/// Extract divergences between block 0 (reference) and each subsequent block.
///
/// Returns `(all_divergences, all_lit_offsets, divergent_literal_offsets)` where
/// `divergent_literal_offsets` is the union of literal offsets across all comparisons.
fn extract_all_divergences(
    sig_inputs: &[(&[Stmt], &[&Stmt])],
    sources: &[&str],
    blocks: &[SourcedBlock],
) -> Result<DivergenceResult> {
    let ref_block = sig_inputs[0].0;
    let mut all_divs = Vec::new();
    let mut all_lit_offsets: Vec<Vec<usize>> = Vec::new();
    let mut divergent_literal_offsets = Vec::new();

    for (i, (other_block, _)) in sig_inputs.iter().enumerate().skip(1) {
        let (divs, lit_offsets) = diff_extract::extract_divergences(
            ref_block,
            other_block,
            sources[blocks[0].source_index],
            sources[blocks[i].source_index],
        )?;

        // Union all literal offsets across comparisons (bug #6 fix).
        for &off in &lit_offsets {
            if !divergent_literal_offsets.contains(&off) {
                divergent_literal_offsets.push(off);
            }
        }
        all_lit_offsets.push(lit_offsets);
        all_divs.push(divs);
    }

    Ok((all_divs, all_lit_offsets, divergent_literal_offsets))
}

/// Stage 1+2 (multi-file): Compute the extraction plan across multiple source files.
///
/// `sources[0]` is the target file. `blocks` are tagged with their source file index.
/// When blocks span multiple files, the scope is forced to `Module` level
/// (to make the function importable).
pub fn plan_extraction_multi(
    sources: &[&str],
    blocks: &[SourcedBlock],
    start_line: usize,
    end_line: usize,
) -> Result<ExtractionPlan> {
    let target_source = sources[0];

    // Parse each source once.
    let parsed: Vec<_> = sources
        .iter()
        .map(|s| parse_python(s).map(ruff_python_parser::Parsed::into_syntax))
        .collect::<Result<Vec<_>>>()?;

    let target_body = &parsed[0].body;
    let is_cross_file = blocks.iter().any(|b| b.source_index != 0);

    // Find the target block's scope info (needed for window_size and all_stores_as_outputs).
    let target_scope_info = scan::find_scopes(target_body, target_source, start_line, end_line);
    let window_size = {
        let target_stmts = normalize::select_stmts(
            target_source,
            target_scope_info.inner_body,
            start_line,
            end_line,
        );
        target_stmts.len()
    };

    let scope_ctx = resolve_scope_context(target_body, target_source, blocks, is_cross_file);
    let (block_slices, after_stmts_per_block) =
        collect_block_contexts(sources, &parsed, blocks, window_size)?;

    // Build sig_inputs by combining block slices with after-statement references.
    let sig_inputs: Vec<(&[Stmt], &[&Stmt])> = block_slices
        .iter()
        .zip(after_stmts_per_block.iter())
        .map(|(block, after)| (*block, after.as_slice()))
        .collect();

    // Safety check: verify block 0 can be extracted into a function.
    if let Err(unsafe_nodes) = safety::check_extractable(sig_inputs[0].0) {
        bail!(
            "{}",
            safety::format_unsafe_error(sources[blocks[0].source_index], &unsafe_nodes)
        );
    }

    let (all_divs, all_lit_offsets, divergent_literal_offsets) =
        extract_all_divergences(&sig_inputs, sources, blocks)?;

    // In class/module scope, all stored variables become outputs.
    // Use the target block's own scope (not placement scope) to decide this.
    let all_stores_as_outputs = target_scope_info.inner_ctx.kind == ScopeKind::Class
        || target_scope_info.inner_ctx.kind == ScopeKind::Module;
    let sig = scope::unify_signatures(
        &sig_inputs,
        &all_divs,
        &all_lit_offsets,
        all_stores_as_outputs,
    );

    // Collect AST node positions and block stores for the plan.
    let ref_node_positions = rewrite::collect_node_positions(sig_inputs[0].0);
    let block_stores: Vec<Vec<String>> = sig_inputs
        .iter()
        .map(|(block, _)| scope::block_stores(block))
        .collect();

    Ok(ExtractionPlan {
        sig,
        scope_ctx,
        ref_node_positions,
        block_stores,
        divergent_literal_offsets,
    })
}

/// Run the full extract-method pipeline (one-shot).
///
/// Runs scan → plan → apply in a single call.
/// For single-file use, pass `sources = &[source]` and `target_file_stem = ""`.
pub fn extract_method_multi(
    sources: &[&str],
    start_line: usize,
    end_line: usize,
    options: &ExtractOptions,
    target_file_stem: &str,
) -> Result<Vec<String>> {
    let all_blocks = scan_all_sources(sources, start_line, end_line)?;
    let plan = plan_extraction_multi(sources, &all_blocks, start_line, end_line)?;
    let func_name = options.func_name.as_deref().unwrap_or("extracted_func_0");
    Ok(rewrite::apply_refactoring_multi(
        sources,
        &all_blocks,
        &plan.ref_node_positions,
        &plan.sig,
        func_name,
        &plan.scope_ctx,
        target_file_stem,
        &plan.divergent_literal_offsets,
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
        let blocks = scan_all_sources(&[source], 1, 2).unwrap();
        assert!(blocks.len() >= 2);

        let plan = plan_extraction_multi(&[source], &blocks, 1, 2).unwrap();
        // Two variable inputs become params; literals diverge and become params too.
        assert!(!plan.sig.params.is_empty());
        assert!(!plan.ref_node_positions.is_empty());
        assert_eq!(plan.scope_ctx.kind, scan::ScopeKind::Module);
    }

    #[test]
    fn plan_extraction_multi_cross_file() {
        let target = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let other = "\
x = 100
y = x + 200
";
        let target_blocks = scan::find_matches(target, 1, 2).unwrap();
        let other_blocks = scan::find_matches_in_file(
            other,
            {
                let (h, _, _) = scan::find_matches_with_hash(target, 1, 2).unwrap();
                h
            },
            2,
        );

        let mut sourced: Vec<SourcedBlock> = target_blocks
            .iter()
            .map(|b| SourcedBlock {
                block: b.clone(),
                source_index: 0,
            })
            .collect();
        sourced.extend(other_blocks.iter().map(|b| SourcedBlock {
            block: b.clone(),
            source_index: 1,
        }));

        let plan = plan_extraction_multi(&[target, other], &sourced, 1, 2).unwrap();
        // Cross-file forces module-level scope.
        assert_eq!(plan.scope_ctx.kind, scan::ScopeKind::Module);
        assert!(!plan.sig.params.is_empty());
        assert_eq!(plan.sig.block_arg_maps.len(), 3); // 2 in target + 1 in other
    }

    #[test]
    fn plan_then_apply_matches_one_shot() {
        let source = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let one_shot =
            extract_method_multi(&[source], 1, 2, &ExtractOptions::default(), "").unwrap();

        let blocks = scan_all_sources(&[source], 1, 2).unwrap();
        let plan = plan_extraction_multi(&[source], &blocks, 1, 2).unwrap();
        let two_stage = rewrite::apply_refactoring_multi(
            &[source],
            &blocks,
            &plan.ref_node_positions,
            &plan.sig,
            "extracted_func_0",
            &plan.scope_ctx,
            "",
            &plan.divergent_literal_offsets,
        );

        assert_eq!(one_shot, two_stage, "Two-stage should match one-shot");
    }
}
