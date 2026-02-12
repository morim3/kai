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
use ruff_python_ast::ModModule;
use ruff_python_parser::Parsed;
use ruff_text_size::Ranged;

use scan::{MatchedBlock, ScopeContext, ScopeKind};
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
    /// Pre-collected AST node positions for block 0.
    pub ref_node_positions: Vec<rewrite::NodePosition>,
    /// All store (assigned) variables per block, in order of first store.
    /// `block_stores[i]` corresponds to `blocks[i]`. Used by interactive mode
    /// to offer additional return value candidates.
    pub block_stores: Vec<Vec<String>>,
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
            "Only {} block(s) found across all files. Need at least 2 matching blocks.",
            all_blocks.len()
        );
    }

    Ok(all_blocks)
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
        .map(|s| parse_python(s).map(|p| p.into_syntax()))
        .collect::<Result<Vec<_>>>()?;

    let target_body = &parsed[0].body;

    // Determine scope context from the target file.
    let mut scope_ctx = scan::find_scope_for_matches(
        target_body,
        target_source,
        start_line,
        end_line,
        &blocks
            .iter()
            .filter(|b| b.source_index == 0)
            .map(|b| b.block.clone())
            .collect::<Vec<_>>(),
    );

    // If any block comes from a different file, force module-level placement.
    let is_cross_file = blocks.iter().any(|b| b.source_index != 0);
    if is_cross_file {
        scope_ctx = ScopeContext {
            kind: ScopeKind::Module,
            body_start_offset: 0,
            indent: String::new(),
            class_def_offset: None,
            parent_indent: None,
        };
    }

    // Get window size from the target file's innermost body.
    let (inner_body, target_block_scope) =
        scan::find_innermost_body(target_body, target_source, start_line, end_line);
    let window_size = {
        let target_stmts = normalize::select_stmts(target_source, inner_body, start_line, end_line);
        target_stmts.len()
    };

    // For each block, find its body and after_block from its own file's AST.
    let mut sig_inputs: Vec<(&[ruff_python_ast::Stmt], &[ruff_python_ast::Stmt])> = Vec::new();
    for sourced in blocks {
        let src = sources[sourced.source_index];
        let body_stmts = &parsed[sourced.source_index].body;
        let body = scan::find_body_for_block(body_stmts, src, sourced.block.start_offset);
        let idx = body
            .iter()
            .position(|s| s.range().start().to_usize() == sourced.block.start_offset)
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

    // Safety check: verify block 0 can be extracted into a function.
    if let Err(unsafe_nodes) = safety::check_extractable(sig_inputs[0].0) {
        bail!(
            "{}",
            safety::format_unsafe_error(sources[blocks[0].source_index], &unsafe_nodes)
        );
    }

    // Extract divergences between block 0 and each other block.
    let mut all_divs = Vec::new();
    if sig_inputs.len() >= 2 {
        let (ref_block, _) = &sig_inputs[0];
        for (i, (other_block, _)) in sig_inputs.iter().enumerate().skip(1) {
            let divs = diff_extract::extract_divergences(
                ref_block,
                other_block,
                sources[blocks[0].source_index],
                sources[blocks[i].source_index],
            )?;
            all_divs.push(divs);
        }
    }

    // In class/module scope, all stored variables become outputs.
    // Class: assignments create class attributes accessible externally.
    // Module: assignments create module globals importable by other modules.
    // Use the target block's own scope (not placement scope) to decide this.
    // E.g., cross_function blocks are inside functions even though placement is module-level.
    let all_stores_as_outputs =
        target_block_scope.kind == ScopeKind::Class || target_block_scope.kind == ScopeKind::Module;
    let sig = scope::unify_signatures(&sig_inputs, &all_divs, all_stores_as_outputs);

    let ref_stmts = sig_inputs[0].0;
    let ref_node_positions = rewrite::collect_node_positions(ref_stmts);

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

/// Stage 1+2: Scan for matches, then compute the extraction plan (single-file).
///
/// This is a convenience wrapper around `plan_extraction_multi` for the single-file case.
pub fn plan_extraction(
    source: &str,
    blocks: &[MatchedBlock],
    start_line: usize,
    end_line: usize,
) -> Result<ExtractionPlan> {
    let sourced: Vec<SourcedBlock> = blocks
        .iter()
        .map(|b| SourcedBlock {
            block: b.clone(),
            source_index: 0,
        })
        .collect();
    plan_extraction_multi(&[source], &sourced, start_line, end_line)
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
