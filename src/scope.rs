use std::collections::HashMap;

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprContext, Stmt};
use ruff_python_stdlib::builtins::is_python_builtin;
use rustc_hash::FxHashSet;

use crate::diff_extract::Divergence;

/// Default Python minor version for builtin detection (Python 3.12).
const DEFAULT_PY_MINOR: u8 = 12;

/// Check if a name is a Python builtin that should be excluded from parameters.
fn is_builtin(name: &str) -> bool {
    is_python_builtin(name, DEFAULT_PY_MINOR, false)
}

/// Default name for the i-th parameter (e.g. `arg_0`, `arg_1`).
pub fn default_param_name(i: usize) -> String {
    format!("arg_{i}")
}

/// Default name for the i-th return value (e.g. `ret_0`, `ret_1`).
pub fn default_return_name(i: usize) -> String {
    format!("ret_{i}")
}

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
/// - `all_stores_as_outputs`: when true, ALL stored variables become outputs
///   regardless of after-block usage. This is needed for class scope, where
///   assignments create class attributes accessible externally.
pub fn analyze_block(
    block: &[Stmt],
    after_block: &[Stmt],
    all_stores_as_outputs: bool,
) -> BlockInterface {
    let mut collector = VarCollector::new();
    for stmt in block {
        collector.visit_stmt(stmt);
    }

    // Inputs: loaded before being stored within the block.
    let inputs = collector.inputs();

    let outputs = if all_stores_as_outputs {
        // In class scope, all stores become class attributes (visible externally).
        collector.stores()
    } else {
        // Determine which variables stored in the block are used after it.
        let mut after_collector = VarCollector::new();
        for stmt in after_block {
            after_collector.visit_stmt(stmt);
        }
        let after_inputs = after_collector.inputs();
        collector
            .stores()
            .into_iter()
            .filter(|name| after_inputs.contains(name))
            .collect()
    };

    BlockInterface { inputs, outputs }
}

/// Analyze a block with after-block statements provided as references.
///
/// This variant accepts `&[&Stmt]` instead of `&[Stmt]`, which is needed when
/// after-block statements are collected from multiple nesting levels (e.g.,
/// statements after control flow structures up to the scope boundary).
pub fn analyze_block_refs(
    block: &[Stmt],
    after_block: &[&Stmt],
    all_stores_as_outputs: bool,
) -> BlockInterface {
    let mut collector = VarCollector::new();
    for stmt in block {
        collector.visit_stmt(stmt);
    }

    let inputs = collector.inputs();

    let outputs = if all_stores_as_outputs {
        collector.stores()
    } else {
        let mut after_collector = VarCollector::new();
        for stmt in after_block {
            after_collector.visit_stmt(stmt);
        }
        let after_inputs = after_collector.inputs();
        collector
            .stores()
            .into_iter()
            .filter(|name| after_inputs.contains(name))
            .collect()
    };

    BlockInterface { inputs, outputs }
}

/// Get all variables that are stored (assigned) within a block, in order of first store.
///
/// Used by the interactive mode to offer additional return value candidates.
pub fn block_stores(block: &[Stmt]) -> Vec<String> {
    let mut collector = VarCollector::new();
    for stmt in block {
        collector.visit_stmt(stmt);
    }
    collector.stores()
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

impl FunctionSignature {
    /// Build the rename map for block 0: original name/literal → new param/return name.
    ///
    /// Entries from `block_arg_maps[0]` map to `params`, then `block_return_maps[0]`
    /// map to `returns` (returns override params for output=input variables).
    pub fn rename_map(&self) -> HashMap<&str, &str> {
        let mut map = HashMap::new();
        if let Some(arg_map) = self.block_arg_maps.first() {
            for (i, original) in arg_map.iter().enumerate() {
                map.insert(original.as_str(), self.params[i].as_str());
            }
        }
        if let Some(ret_map) = self.block_return_maps.first() {
            for (i, original) in ret_map.iter().enumerate() {
                map.insert(original.as_str(), self.returns[i].as_str());
            }
        }
        map
    }
}

/// Collect literal divergences across all blocks into a table of per-parameter values.
///
/// `all_divergences[i]` contains divergences between block 0 and block `i+1`.
/// Returns a `Vec<Vec<String>>` where `result[param_idx][block_idx]` is the literal
/// value for that parameter in that block.
fn collect_literal_params(all_divergences: &[Vec<Divergence>]) -> Vec<Vec<String>> {
    let Some(first_divs) = all_divergences.first() else {
        return Vec::new();
    };

    let mut params: Vec<Vec<String>> = Vec::new();

    for div in first_divs {
        if let Divergence::Literal(val_0, val_1) = div {
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
                        Divergence::Literal(_, v) => Some(v),
                        Divergence::Name(..) => None,
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
/// `all_stores_as_outputs`: when true, all stored variables become outputs (for class scope).
pub fn unify_signatures(
    blocks: &[(&[Stmt], &[&Stmt])],
    all_divergences: &[Vec<Divergence>],
    all_stores_as_outputs: bool,
) -> FunctionSignature {
    let interfaces: Vec<BlockInterface> = blocks
        .iter()
        .map(|(block, after)| analyze_block_refs(block, after, all_stores_as_outputs))
        .collect();

    // All blocks should have the same number of inputs/outputs (structurally identical).
    let param_count = interfaces.first().map_or(0, |i| i.inputs.len());

    let literal_param_values = collect_literal_params(all_divergences);

    let lit_count = literal_param_values.len();
    let total_params = param_count + lit_count;

    let params: Vec<String> = (0..total_params).map(default_param_name).collect();

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
                default_param_name(input_idx)
            } else {
                let name = default_return_name(ret_counter);
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
                    if !stored_set.contains(name.as_str())
                        && !is_builtin(name)
                        && input_set.insert(name.as_str())
                    {
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

impl VarCollector {
    /// Handle a comprehension with its own scope (Python 3 semantics).
    /// The first generator's `iter` is evaluated in the enclosing scope.
    /// Targets and all other expressions are in the comprehension's scope.
    fn visit_comprehension_scoped(
        &mut self,
        generators: &[ruff_python_ast::Comprehension],
        visit_output: impl FnOnce(&mut VarCollector),
    ) {
        // First generator's iter is evaluated in the enclosing scope.
        if let Some(first) = generators.first() {
            self.visit_expr(&first.iter);
        }

        let mut inner = VarCollector::new();
        for (i, comp) in generators.iter().enumerate() {
            if i > 0 {
                // Subsequent generators' iter is in the comprehension scope.
                inner.visit_expr(&comp.iter);
            }
            // Target is a Store in the comprehension scope.
            inner.visit_expr(&comp.target);
            for if_clause in &comp.ifs {
                inner.visit_expr(if_clause);
            }
        }
        visit_output(&mut inner);

        // Merge free variables as loads in the enclosing scope.
        for name in inner.inputs() {
            self.events.push((name, VarAction::Load));
        }
    }
}

impl<'a> Visitor<'a> for VarCollector {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(name) => {
                let action = match name.ctx {
                    ExprContext::Load => VarAction::Load,
                    ExprContext::Store | ExprContext::Del => VarAction::Store,
                    ExprContext::Invalid => return,
                };
                self.events.push((name.id.to_string(), action));
            }
            // Lambda creates a new scope. Use a nested collector so params
            // don't leak into the outer scope. Only free variables propagate.
            Expr::Lambda(lambda) => {
                // Default values are evaluated in the outer scope.
                if let Some(ref params) = lambda.parameters {
                    for param in params
                        .posonlyargs
                        .iter()
                        .chain(&params.args)
                        .chain(&params.kwonlyargs)
                    {
                        if let Some(ref default) = param.default {
                            self.visit_expr(default);
                        }
                    }
                }
                // Nested collector: register params as stores, walk body.
                let mut inner = VarCollector::new();
                if let Some(ref params) = lambda.parameters {
                    for param in params
                        .posonlyargs
                        .iter()
                        .chain(&params.args)
                        .chain(&params.kwonlyargs)
                    {
                        inner
                            .events
                            .push((param.parameter.name.to_string(), VarAction::Store));
                    }
                    if let Some(ref vararg) = params.vararg {
                        inner
                            .events
                            .push((vararg.name.to_string(), VarAction::Store));
                    }
                    if let Some(ref kwarg) = params.kwarg {
                        inner
                            .events
                            .push((kwarg.name.to_string(), VarAction::Store));
                    }
                }
                inner.visit_expr(&lambda.body);
                // Merge free variables as loads in the outer scope.
                for name in inner.inputs() {
                    self.events.push((name, VarAction::Load));
                }
            }
            // Comprehensions create their own scope in Python 3.
            // Iteration variables don't leak to the enclosing scope.
            Expr::ListComp(comp) => {
                self.visit_comprehension_scoped(&comp.generators, |inner| {
                    inner.visit_expr(&comp.elt);
                });
            }
            Expr::SetComp(comp) => {
                self.visit_comprehension_scoped(&comp.generators, |inner| {
                    inner.visit_expr(&comp.elt);
                });
            }
            Expr::DictComp(comp) => {
                self.visit_comprehension_scoped(&comp.generators, |inner| {
                    inner.visit_expr(&comp.key);
                    inner.visit_expr(&comp.value);
                });
            }
            Expr::Generator(generator) => {
                self.visit_comprehension_scoped(&generator.generators, |inner| {
                    inner.visit_expr(&generator.elt);
                });
            }
            _ => {
                walk_expr(self, expr);
            }
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
        let iface = analyze_block(&stmts, &[], false);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, Vec::<String>::new());
    }

    #[test]
    fn input_not_duplicated_when_loaded_twice() {
        let stmts = parse_stmts("y = x + x");
        let iface = analyze_block(&stmts, &[], false);
        assert_eq!(iface.inputs, vec!["x"]);
    }

    #[test]
    fn store_then_load_is_not_input() {
        // `a = 1; b = a + 2` — `a` is stored first, so it's not an input.
        let stmts = parse_stmts("a = 1\nb = a + 2");
        let iface = analyze_block(&stmts, &[], false);
        assert_eq!(iface.inputs, Vec::<String>::new());
    }

    #[test]
    fn outputs_used_after_block() {
        let block = parse_stmts("result = x + 1");
        let after = parse_stmts("print(result)");
        let iface = analyze_block(&block, &after, false);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, vec!["result"]);
    }

    #[test]
    fn outputs_not_used_after_block() {
        let block = parse_stmts("temp = x + 1");
        let after = parse_stmts("print(42)");
        let iface = analyze_block(&block, &after, false);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, Vec::<String>::new());
    }

    #[test]
    fn aug_assign_is_both_input_and_store() {
        // `x += 1` — x is both loaded and stored.
        let block = parse_stmts("x += 1");
        let after = parse_stmts("print(x)");
        let iface = analyze_block(&block, &after, false);
        assert_eq!(iface.inputs, vec!["x"]);
        assert_eq!(iface.outputs, vec!["x"]);
    }

    #[test]
    fn multiple_inputs_and_outputs() {
        let block = parse_stmts("c = a + b\nd = c * 2");
        let after = parse_stmts("print(c, d)");
        let iface = analyze_block(&block, &after, false);
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
        let divs =
            crate::diff_extract::extract_divergences(&block_a, &block_b, src_a, src_b).unwrap();

        let after_a_refs: Vec<&Stmt> = after_a.iter().collect();
        let after_b_refs: Vec<&Stmt> = after_b.iter().collect();
        let sig = unify_signatures(
            &[
                (block_a.as_slice(), &after_a_refs),
                (block_b.as_slice(), &after_b_refs),
            ],
            &[divs],
            false,
        );

        assert_eq!(sig.params, vec!["arg_0", "arg_1"]);
        assert_eq!(sig.returns, vec!["ret_0"]);
        assert_eq!(sig.block_arg_maps, vec![vec!["a", "b"], vec!["x", "y"]]);
        assert_eq!(sig.block_return_maps, vec![vec!["c"], vec!["z"]]);
    }

    #[test]
    fn collect_literal_params_empty() {
        let result = collect_literal_params(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn collect_literal_params_no_literals() {
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
        let divs_ab =
            crate::diff_extract::extract_divergences(&block_a, &block_b, src_a, src_b).unwrap();
        let divs_ac =
            crate::diff_extract::extract_divergences(&block_a, &block_c, src_a, src_c).unwrap();

        let empty_refs: Vec<&Stmt> = Vec::new();
        let sig = unify_signatures(
            &[
                (block_a.as_slice(), &empty_refs),
                (block_b.as_slice(), &empty_refs),
                (block_c.as_slice(), &empty_refs),
            ],
            &[divs_ab, divs_ac],
            false,
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
        let iface = analyze_block(&block, &[], false);
        assert_eq!(
            iface.inputs,
            vec!["a"],
            "a should be an input since RHS loads it first"
        );
    }

    #[test]
    fn builtins_excluded_from_inputs() {
        // `print(x)` — `print` is a builtin and should not appear as input.
        let block = parse_stmts("y = len(x)\nprint(y)");
        let iface = analyze_block(&block, &[], false);
        assert_eq!(iface.inputs, vec!["x"], "builtins len/print excluded");
    }

    #[test]
    fn tuple_unpacking_stores() {
        // `a, b = func()` — both a and b are stores.
        let block = parse_stmts("a, b = func()");
        let after = parse_stmts("print(a, b)");
        let iface = analyze_block(&block, &after, false);
        assert_eq!(iface.inputs, vec!["func"], "func is loaded");
        assert_eq!(
            iface.outputs,
            vec!["a", "b"],
            "both unpacked vars are outputs"
        );
    }

    #[test]
    fn del_treated_as_store() {
        // `del x` — ExprContext::Del is treated as Store, so x should not
        // appear as input (it's "defined" by del). If x was loaded before
        // being deleted, it's an input.
        let block = parse_stmts("y = x + 1\ndel x");
        let iface = analyze_block(&block, &[], false);
        assert_eq!(iface.inputs, vec!["x"], "x loaded before del");
    }

    #[test]
    fn for_loop_target_is_store() {
        // `for i in items:` — i is a store, items is a load.
        let block = parse_stmts("for i in items:\n    pass");
        let after = parse_stmts("print(i)");
        let iface = analyze_block(&block, &after, false);
        assert_eq!(iface.inputs, vec!["items"]);
        assert_eq!(iface.outputs, vec!["i"]);
    }

    #[test]
    fn lambda_params_excluded_from_inputs() {
        // `lambda x: x + y` — x is a param (not input), y is a free variable (input).
        let block = parse_stmts("f = lambda x: x + y");
        let iface = analyze_block(&block, &[], false);
        assert_eq!(iface.inputs, vec!["y"], "lambda param x must not be input");
    }

    #[test]
    fn lambda_default_is_input() {
        // `lambda x, y=z: x + y` — z is evaluated in the outer scope.
        let block = parse_stmts("f = lambda x, y=z: x + y");
        let iface = analyze_block(&block, &[], false);
        assert_eq!(iface.inputs, vec!["z"], "default value z is an input");
    }

    #[test]
    fn class_scope_all_stores_as_outputs() {
        // In class scope, all stores become outputs (class attributes).
        let block = parse_stmts("x = 1\ny = x + 2");
        let after = parse_stmts("a = 10");
        // With all_stores_as_outputs=false, only stores used in after_block are outputs.
        let iface_normal = analyze_block(&block, &after, false);
        assert_eq!(iface_normal.outputs, Vec::<String>::new());
        // With all_stores_as_outputs=true, all stores become outputs.
        let iface_class = analyze_block(&block, &after, true);
        assert_eq!(iface_class.outputs, vec!["x", "y"]);
    }

    #[test]
    fn block_stores_returns_all_stores() {
        let block = parse_stmts("a = 1\nb = a + 2\nc = b * 3");
        let stores = block_stores(&block);
        assert_eq!(stores, vec!["a", "b", "c"]);
    }
}
