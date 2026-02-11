use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprContext, Stmt};

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
    let after_loads = after_collector.all_loads();

    let outputs: Vec<String> = collector
        .stores()
        .into_iter()
        .filter(|name| after_loads.contains(name))
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
    /// For each matched block: the mapping from param name -> original variable name.
    pub block_arg_maps: Vec<Vec<String>>,
    /// For each matched block: the mapping from return name -> original variable name.
    pub block_return_maps: Vec<Vec<String>>,
}

/// Given multiple structurally equivalent blocks and their after-blocks,
/// compute a unified function signature and variable mappings.
///
/// Each entry in `blocks` is `(block_stmts, after_stmts)`.
pub fn unify_signatures(blocks: &[(&[Stmt], &[Stmt])]) -> FunctionSignature {
    let interfaces: Vec<BlockInterface> = blocks
        .iter()
        .map(|(block, after)| analyze_block(block, after))
        .collect();

    // All blocks should have the same number of inputs/outputs (structurally identical).
    let param_count = interfaces.first().map_or(0, |i| i.inputs.len());
    let return_count = interfaces.first().map_or(0, |i| i.outputs.len());

    let params: Vec<String> = (0..param_count).map(|i| format!("arg_{i}")).collect();
    let returns: Vec<String> = (0..return_count).map(|i| format!("ret_{i}")).collect();

    let block_arg_maps: Vec<Vec<String>> = interfaces.iter().map(|i| i.inputs.clone()).collect();
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
        let mut stored = Vec::new();
        let mut inputs = Vec::new();
        for (name, action) in &self.events {
            match action {
                VarAction::Store => {
                    if !stored.contains(name) {
                        stored.push(name.clone());
                    }
                }
                VarAction::Load => {
                    if !stored.contains(name) && !inputs.contains(name) {
                        inputs.push(name.clone());
                    }
                }
            }
        }
        inputs
    }

    /// All variables that are stored in this block (in order of first store).
    fn stores(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (name, action) in &self.events {
            if *action == VarAction::Store && !result.contains(name) {
                result.push(name.clone());
            }
        }
        result
    }

    /// All variables that are loaded in this block.
    fn all_loads(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (name, action) in &self.events {
            if *action == VarAction::Load && !result.contains(name) {
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
    use ruff_python_parser::parse_module;

    /// Parse source and return the body statements.
    fn parse_stmts(source: &str) -> Vec<Stmt> {
        parse_module(source).unwrap().into_syntax().body
    }

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

        let sig = unify_signatures(&[
            (block_a.as_slice(), after_a.as_slice()),
            (block_b.as_slice(), after_b.as_slice()),
        ]);

        assert_eq!(sig.params, vec!["arg_0", "arg_1"]);
        assert_eq!(sig.returns, vec!["ret_0"]);
        assert_eq!(sig.block_arg_maps, vec![vec!["a", "b"], vec!["x", "y"]]);
        assert_eq!(sig.block_return_maps, vec![vec!["c"], vec!["z"]]);
    }

    #[test]
    fn load_before_store_in_same_statement() {
        // `a = a + 1` — `a` on the RHS is loaded before being stored on the LHS.
        let block = parse_stmts("a = a + 1");
        let iface = analyze_block(&block, &[]);
        assert_eq!(iface.inputs, vec!["a"], "a should be an input since RHS loads it first");
    }
}
