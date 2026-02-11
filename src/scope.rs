use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprContext, Stmt};
use rustc_hash::FxHashSet;

/// The interface of an extracted function block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInterface {
    /// Variables that are read (Load) but not previously written (Store) within the block.
    /// These become function parameters. Ordered by first appearance.
    pub inputs: Vec<String>,
    /// Variables that are written (Store) within the block and used after the block.
    /// These become return values. Ordered by first appearance.
    pub outputs: Vec<String>,
}

/// Analyze a block of statements to determine inputs and outputs.
///
/// - `block`: the statements being extracted.
/// - `after_block`: statements that come after the block in the same scope
///   (used to determine which stored variables are live-out).
pub fn analyze_block(block: &[Stmt], after_block: &[Stmt]) -> BlockInterface {
    let mut collector = VarCollector::new();
    for stmt in block {
        collector.visit_stmt(stmt);
    }

    // Inputs: loaded before being stored within the block.
    let inputs = collector.inputs();

    // Determine which variables stored in the block are used after it.
    let mut after_collector = VarCollector::new();
    for stmt in after_block {
        after_collector.visit_stmt(stmt);
    }
    // Only variables that are loaded *before* being stored in the after-block
    // are truly live-out. Using inputs() (not all_loads()) avoids false positives
    // when the after-block overwrites a variable before reading it.
    let after_inputs = after_collector.inputs();

    let outputs: Vec<String> = collector
        .stores()
        .into_iter()
        .filter(|name| after_inputs.contains(name))
        .collect();

    BlockInterface { inputs, outputs }
}

/// Mapping of variables across structurally equivalent blocks.
/// Maps each block's variable names to a common parameter name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    /// Parameter names for the extracted function (e.g., `arg_0`, `arg_1`).
    pub params: Vec<String>,
    /// Return variable names for the extracted function.
    pub returns: Vec<String>,
    /// For each matched block: the mapping from param name -> original variable name/value.
    pub block_arg_maps: Vec<Vec<String>>,
    /// For each matched block: the mapping from return name -> original variable name.
    pub block_return_maps: Vec<Vec<String>>,
}

/// Collect literal divergences across all blocks into a table of per-parameter values.
///
/// `all_divergences[i]` contains divergences between block 0 and block `i+1`.
/// Returns a `Vec<Vec<String>>` where `result[param_idx][block_idx]` is the literal
/// value for that parameter in that block.
fn collect_literal_params(
    all_divergences: &[Vec<crate::diff_extract::Divergence>],
) -> Vec<Vec<String>> {
    let first_divs = match all_divergences.first() {
        Some(d) => d,
        None => return Vec::new(),
    };

    let mut params: Vec<Vec<String>> = Vec::new();

    for div in first_divs {
        if let crate::diff_extract::Divergence::Literal(val_0, val_1) = div {
            let num_blocks = all_divergences.len() + 1; // +1 for block 0 itself
            let mut per_block = Vec::with_capacity(num_blocks);
            per_block.push(val_0.clone()); // block 0
            per_block.push(val_1.clone()); // block 1

            // For blocks 2..N, find the matching literal divergence by ordinal position.
            let current_lit_idx = params.len();
            for other_divs in all_divergences.iter().skip(1) {
                let value = other_divs
                    .iter()
                    .filter_map(|od| match od {
                        crate::diff_extract::Divergence::Literal(_, v) => Some(v),
                        _ => None,
                    })
                    .nth(current_lit_idx)
                    .cloned()
                    .unwrap_or_else(|| val_0.clone()); // fallback: reuse block 0's value
                per_block.push(value);
            }

            params.push(per_block);
        }
    }

    params
}

/// Given multiple structurally equivalent blocks and their after-blocks,
/// compute a unified function signature and variable mappings.
///
/// Each entry in `blocks` is `(block_stmts, after_stmts)`.
/// `divergences` contains the structural differences between each block and block 0.
pub fn unify_signatures(
    blocks: &[(&[Stmt], &[Stmt])],
    all_divergences: &[Vec<crate::diff_extract::Divergence>],
    custom_params: &Option<Vec<String>>,
) -> FunctionSignature {
    let interfaces: Vec<BlockInterface> = blocks
        .iter()
        .map(|(block, after)| analyze_block(block, after))
        .collect();

    // All blocks should have the same number of inputs/outputs (structurally identical).
    let param_count = interfaces.first().map_or(0, |i| i.inputs.len());

    let literal_param_values = collect_literal_params(all_divergences);

    let lit_count = literal_param_values.len();
    let total_params = param_count + lit_count;

    let params: Vec<String> = if let Some(names) = custom_params {
        (0..total_params)
            .map(|i| names.get(i).cloned().unwrap_or_else(|| format!("arg_{i}")))
            .collect()
    } else {
        (0..total_params).map(|i| format!("arg_{i}")).collect()
    };

    // For outputs that are also inputs, reuse the corresponding arg_N name
    // instead of introducing a separate ret_N. This avoids double-renaming
    // conflicts in generate_function_def().
    let ref_iface = &interfaces[0];
    let mut ret_counter = 0;
    let returns: Vec<String> = ref_iface
        .outputs
        .iter()
        .map(|out_var| {
            if let Some(input_idx) = ref_iface.inputs.iter().position(|inp| inp == out_var) {
                format!("arg_{input_idx}")
            } else {
                let name = format!("ret_{ret_counter}");
                ret_counter += 1;
                name
            }
        })
        .collect();

    let num_blocks = blocks.len();
    let mut block_arg_maps: Vec<Vec<String>> = Vec::with_capacity(num_blocks);

    for block_idx in 0..num_blocks {
        let mut args: Vec<String> = if block_idx < interfaces.len() {
            interfaces[block_idx].inputs.clone()
        } else {
            Vec::new()
        };
        // Append literal values for this block.
        for lit_vals in &literal_param_values {
            if block_idx < lit_vals.len() {
                args.push(lit_vals[block_idx].clone());
            }
        }
        block_arg_maps.push(args);
    }

    let block_return_maps: Vec<Vec<String>> =
        interfaces.iter().map(|i| i.outputs.clone()).collect();

    FunctionSignature {
        params,
        returns,
        block_arg_maps,
        block_return_maps,
    }
}

/// Collects variable loads and stores in order of first appearance.
struct VarCollector {
    /// (name, action) in order of encounter. Action is Load or Store.
    events: Vec<(String, VarAction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarAction {
    Load,
    Store,
}

impl VarCollector {
    fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Variables that are loaded before being stored (inputs).
    fn inputs(&self) -> Vec<String> {
        let mut stored_set = FxHashSet::default();
        let mut input_set = FxHashSet::default();
        let mut inputs = Vec::new();
        for (name, action) in &self.events {
            match action {
                VarAction::Store => {
                    stored_set.insert(name.as_str());
                }
                VarAction::Load => {
                    if !stored_set.contains(name.as_str()) && input_set.insert(name.as_str()) {
                        inputs.push(name.clone());
                    }
                }
            }
        }
        inputs
    }

    /// All variables that are stored in this block (in order of first store).
    fn stores(&self) -> Vec<String> {
        let mut seen = FxHashSet::default();
        let mut result = Vec::new();
        for (name, action) in &self.events {
            if *action == VarAction::Store && seen.insert(name.as_str()) {
                result.push(name.clone());
            }
        }
        result
    }
}

impl<'a> Visitor<'a> for VarCollector {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr {
            let action = match name.ctx {
                ExprContext::Load => VarAction::Load,
                ExprContext::Store | ExprContext::Del => VarAction::Store,
                ExprContext::Invalid => return,
            };
            self.events.push((name.id.to_string(), action));
        } else {
            walk_expr(self, expr);
        }
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        // For assignments, we must visit the value (RHS) before the target (LHS)
        // so that `a = a + 1` correctly records the Load of `a` before the Store.
        if let Stmt::Assign(assign) = stmt {
            self.visit_expr(&assign.value);
            for target in &assign.targets {
                self.visit_expr(target);
            }
        } else if let Stmt::AugAssign(aug) = stmt {
            // `a += 1` both loads and stores `a`.
            self.visit_expr(&aug.value);
            // The target of augmented assign is both loaded and stored.
            // Record a Load first, then a Store.
            if let Expr::Name(name) = &*aug.target {
                self.events.push((name.id.to_string(), VarAction::Load));
                self.events.push((name.id.to_string(), VarAction::Store));
            } else {
                self.visit_expr(&aug.target);
            }
        } else {
            walk_stmt(self, stmt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_stmts;

    #[test]
    fn simple_inputs() {
        // `y = x + 1` — x is loaded, y is stored.
        let stmts = parse_stmts("y = x + 1");
        let iface = analyze_block(&stmts, &[]);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, Vec::<String>::new());
    }

    #[test]
    fn input_not_duplicated_when_loaded_twice() {
        let stmts = parse_stmts("y = x + x");
        let iface = analyze_block(&stmts, &[]);
        assert_eq!(iface.inputs, vec!["x"]);
    }

    #[test]
    fn store_then_load_is_not_input() {
        // `a = 1; b = a + 2` — `a` is stored first, so it's not an input.
        let stmts = parse_stmts("a = 1\nb = a + 2");
        let iface = analyze_block(&stmts, &[]);
        assert_eq!(iface.inputs, Vec::<String>::new());
    }

    #[test]
    fn outputs_used_after_block() {
        let block = parse_stmts("result = x + 1");
        let after = parse_stmts("print(result)");
        let iface = analyze_block(&block, &after);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, vec!["result"]);
    }

    #[test]
    fn outputs_not_used_after_block() {
        let block = parse_stmts("temp = x + 1");
        let after = parse_stmts("print(42)");
        let iface = analyze_block(&block, &after);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, Vec::<String>::new());
    }

    #[test]
    fn aug_assign_is_both_input_and_store() {
        // `x += 1` — x is both loaded and stored.
        let block = parse_stmts("x += 1");
        let after = parse_stmts("print(x)");
        let iface = analyze_block(&block, &after);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, vec!["x"]);
    }

    #[test]
    fn multiple_inputs_and_outputs() {
        let block = parse_stmts("c = a + b\nd = c * 2");
        let after = parse_stmts("print(c, d)");
        let iface = analyze_block(&block, &after);
        assert_eq!(iface.inputs, vec!["a", "b"]);
        assert_eq!(iface.outputs, vec!["c", "d"]);
    }

    #[test]
    fn unify_two_blocks() {
        let block_a = parse_stmts("c = a + b");
        let block_b = parse_stmts("z = x + y");
        let after_a = parse_stmts("print(c)");
        let after_b = parse_stmts("print(z)");

        let src_a = "c = a + b";
        let src_b = "z = x + y";
        let divs = crate::diff_extract::extract_divergences(&block_a, &block_b, src_a, src_b);

        let sig = unify_signatures(
            &[
                (block_a.as_slice(), after_a.as_slice()),
                (block_b.as_slice(), after_b.as_slice()),
            ],
            &[divs],
            &None,
        );

        assert_eq!(sig.params, vec!["arg_0", "arg_1"]);
        assert_eq!(sig.returns, vec!["ret_0"]);
        assert_eq!(sig.block_arg_maps, vec![vec!["a", "b"], vec!["x", "y"]]);
        assert_eq!(sig.block_return_maps, vec![vec!["c"], vec!["z"]]);
    }

    #[test]
    fn unify_with_custom_params() {
        let block_a = parse_stmts("c = a + b");
        let block_b = parse_stmts("z = x + y");
        let after_a = parse_stmts("print(c)");
        let after_b = parse_stmts("print(z)");

        let src_a = "c = a + b";
        let src_b = "z = x + y";
        let divs = crate::diff_extract::extract_divergences(&block_a, &block_b, src_a, src_b);

        let custom = Some(vec!["lhs".to_string(), "rhs".to_string()]);
        let sig = unify_signatures(
            &[
                (block_a.as_slice(), after_a.as_slice()),
                (block_b.as_slice(), after_b.as_slice()),
            ],
            &[divs],
            &custom,
        );

        assert_eq!(sig.params, vec!["lhs", "rhs"]);
        // Returns still use auto-generated names
        assert_eq!(sig.returns, vec!["ret_0"]);
    }

    #[test]
    fn collect_literal_params_empty() {
        let result = collect_literal_params(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn collect_literal_params_no_literals() {
        use crate::diff_extract::Divergence;
        let divs = vec![vec![Divergence::Name("a".into(), "b".into())]];
        let result = collect_literal_params(&divs);
        assert!(result.is_empty());
    }

    #[test]
    fn unify_three_blocks_with_literal_divergence() {
        // Three blocks: `x = 1 + 2`, `x = 10 + 20`, `x = 100 + 200`
        let src_a = "x = 1 + 2";
        let src_b = "x = 10 + 20";
        let src_c = "x = 100 + 200";
        let block_a = parse_stmts(src_a);
        let block_b = parse_stmts(src_b);
        let block_c = parse_stmts(src_c);

        // Divergences: block_a vs block_b, block_a vs block_c
        let divs_ab = crate::diff_extract::extract_divergences(&block_a, &block_b, src_a, src_b);
        let divs_ac = crate::diff_extract::extract_divergences(&block_a, &block_c, src_a, src_c);

        let sig = unify_signatures(
            &[
                (block_a.as_slice(), &[]),
                (block_b.as_slice(), &[]),
                (block_c.as_slice(), &[]),
            ],
            &[divs_ab, divs_ac],
            &None,
        );

        // 2 literal divergences become 2 params (no variable inputs)
        assert_eq!(sig.params, vec!["arg_0", "arg_1"]);
        // Each block's arg map should contain its own literal values
        assert_eq!(
            sig.block_arg_maps,
            vec![vec!["1", "2"], vec!["10", "20"], vec!["100", "200"],]
        );
    }

    #[test]
    fn load_before_store_in_same_statement() {
        // `a = a + 1` — `a` on the RHS is loaded before being stored on the LHS.
        let block = parse_stmts("a = a + 1");
        let iface = analyze_block(&block, &[]);
        assert_eq!(
            iface.inputs,
            vec!["a"],
            "a should be an input since RHS loads it first"
        );
    }
}
