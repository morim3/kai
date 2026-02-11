use std::hash::{Hash, Hasher};

use anyhow::{Result, bail};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{
    BoolOp, CmpOp, Expr, ExprContext, Operator, Stmt, UnaryOp,
};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;
use rustc_hash::FxHasher;

/// Compute the structural hash of a slice of AST statements.
pub fn hash_stmts(stmts: &[Stmt]) -> u64 {
    let mut visitor = NormalizeVisitor::new();
    for stmt in stmts {
        visitor.visit_stmt(stmt);
    }
    visitor.finish()
}

/// Compute the structural hash of a collection of statement references.
pub fn hash_stmt_refs(stmts: &[&Stmt]) -> u64 {
    let mut visitor = NormalizeVisitor::new();
    for stmt in stmts {
        visitor.visit_stmt(stmt);
    }
    visitor.finish()
}

/// Extract the statements covering `start_line..=end_line` (1-based) from source.
/// Returns a hash of the structurally-normalized AST for those statements.
pub fn hash_block(source: &str, start_line: usize, end_line: usize) -> Result<u64> {
    if start_line == 0 || end_line == 0 || start_line > end_line {
        bail!("Invalid line range: {start_line}..={end_line}");
    }

    let parsed = parse_module(source).map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;

    let stmts = select_stmts(source, &parsed.syntax().body, start_line, end_line);
    if stmts.is_empty() {
        bail!("No statements found in range {start_line}..={end_line}");
    }

    Ok(hash_stmt_refs(&stmts))
}

/// Select statements whose line range overlaps with the given 1-based line range.
pub fn select_stmts<'a>(
    source: &str,
    body: &'a [Stmt],
    start_line: usize,
    end_line: usize,
) -> Vec<&'a Stmt> {
    body.iter()
        .filter(|stmt| {
            let range = stmt.range();
            let stmt_start_line = line_of_offset(source, range.start().to_usize());
            let stmt_end_line = line_of_offset(source, range.end().to_usize().saturating_sub(1));
            stmt_start_line <= end_line && stmt_end_line >= start_line
        })
        .collect()
}

/// Convert a byte offset to a 1-based line number.
pub fn line_of_offset(source: &str, offset: usize) -> usize {
    let offset = offset.min(source.len());
    source[..offset].chars().filter(|&c| c == '\n').count() + 1
}

/// A visitor that walks the AST and produces a structural hash.
///
/// - Variable names (`Expr::Name`) are replaced with positional tags (VAR_0, VAR_1, ...).
/// - Literals are replaced with a constant token.
/// - The structure (node types, operators, nesting) is fully hashed.
struct NormalizeVisitor {
    hasher: FxHasher,
    /// Maps original variable names to sequential IDs (insertion order).
    var_map: Vec<String>,
}

impl NormalizeVisitor {
    fn new() -> Self {
        Self {
            hasher: FxHasher::default(),
            var_map: Vec::new(),
        }
    }

    fn finish(&self) -> u64 {
        self.hasher.finish()
    }

    /// Get or assign a sequential ID for a variable name.
    fn var_id(&mut self, name: &str) -> usize {
        if let Some(pos) = self.var_map.iter().position(|n| n == name) {
            pos
        } else {
            let id = self.var_map.len();
            self.var_map.push(name.to_string());
            id
        }
    }

    fn hash_tag(&mut self, tag: &str) {
        tag.hash(&mut self.hasher);
    }

    fn hash_usize(&mut self, val: usize) {
        val.hash(&mut self.hasher);
    }
}

impl<'a> Visitor<'a> for NormalizeVisitor {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        // Hash the discriminant (statement kind) then recurse.
        let tag = match stmt {
            Stmt::FunctionDef(_) => "FunctionDef",
            Stmt::ClassDef(_) => "ClassDef",
            Stmt::Return(_) => "Return",
            Stmt::Delete(_) => "Delete",
            Stmt::Assign(_) => "Assign",
            Stmt::AugAssign(_) => "AugAssign",
            Stmt::AnnAssign(_) => "AnnAssign",
            Stmt::TypeAlias(_) => "TypeAlias",
            Stmt::For(_) => "For",
            Stmt::While(_) => "While",
            Stmt::If(_) => "If",
            Stmt::With(_) => "With",
            Stmt::Match(_) => "Match",
            Stmt::Raise(_) => "Raise",
            Stmt::Try(_) => "Try",
            Stmt::Assert(_) => "Assert",
            Stmt::Import(_) => "Import",
            Stmt::ImportFrom(_) => "ImportFrom",
            Stmt::Global(_) => "Global",
            Stmt::Nonlocal(_) => "Nonlocal",
            Stmt::Expr(_) => "ExprStmt",
            Stmt::Pass(_) => "Pass",
            Stmt::Break(_) => "Break",
            Stmt::Continue(_) => "Continue",
            Stmt::IpyEscapeCommand(_) => "IpyEscapeCommand",
        };
        self.hash_tag(tag);
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            // Variable names: normalize to positional IDs.
            Expr::Name(name) => {
                self.hash_tag("Name");
                let id = self.var_id(&name.id);
                self.hash_usize(id);
                // Hash context (Load/Store/Del) so that `x = ...` and `... = x` differ.
                self.visit_expr_context(&name.ctx);
            }

            // All literals: hash as a single CONSTANT token.
            Expr::NumberLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_) => {
                self.hash_tag("CONSTANT");
            }

            // For everything else, hash the node kind and recurse.
            _ => {
                let tag = match expr {
                    Expr::BoolOp(_) => "BoolOp",
                    Expr::Named(_) => "Named",
                    Expr::BinOp(_) => "BinOp",
                    Expr::UnaryOp(_) => "UnaryOp",
                    Expr::Lambda(_) => "Lambda",
                    Expr::If(_) => "IfExpr",
                    Expr::Dict(_) => "Dict",
                    Expr::Set(_) => "Set",
                    Expr::ListComp(_) => "ListComp",
                    Expr::SetComp(_) => "SetComp",
                    Expr::DictComp(_) => "DictComp",
                    Expr::Generator(_) => "Generator",
                    Expr::Await(_) => "Await",
                    Expr::Yield(_) => "Yield",
                    Expr::YieldFrom(_) => "YieldFrom",
                    Expr::Compare(_) => "Compare",
                    Expr::Call(_) => "Call",
                    Expr::FString(_) => "FString",
                    Expr::TString(_) => "TString",
                    Expr::Attribute(_) => "Attribute",
                    Expr::Subscript(_) => "Subscript",
                    Expr::Starred(_) => "Starred",
                    Expr::List(_) => "List",
                    Expr::Tuple(_) => "Tuple",
                    Expr::Slice(_) => "Slice",
                    Expr::IpyEscapeCommand(_) => "IpyEscapeCommand",
                    // Already handled above.
                    Expr::Name(_)
                    | Expr::NumberLiteral(_)
                    | Expr::StringLiteral(_)
                    | Expr::BytesLiteral(_)
                    | Expr::BooleanLiteral(_)
                    | Expr::NoneLiteral(_)
                    | Expr::EllipsisLiteral(_) => unreachable!(),
                };
                self.hash_tag(tag);
                walk_expr(self, expr);
            }
        }
    }

    fn visit_expr_context(&mut self, ctx: &'a ExprContext) {
        let tag = match ctx {
            ExprContext::Load => "Load",
            ExprContext::Store => "Store",
            ExprContext::Del => "Del",
            ExprContext::Invalid => "Invalid",
        };
        self.hash_tag(tag);
    }

    fn visit_bool_op(&mut self, op: &'a BoolOp) {
        let tag = match op {
            BoolOp::And => "And",
            BoolOp::Or => "Or",
        };
        self.hash_tag(tag);
    }

    fn visit_operator(&mut self, op: &'a Operator) {
        let tag = match op {
            Operator::Add => "Add",
            Operator::Sub => "Sub",
            Operator::Mult => "Mult",
            Operator::Div => "Div",
            Operator::FloorDiv => "FloorDiv",
            Operator::Mod => "Mod",
            Operator::Pow => "Pow",
            Operator::LShift => "LShift",
            Operator::RShift => "RShift",
            Operator::BitOr => "BitOr",
            Operator::BitXor => "BitXor",
            Operator::BitAnd => "BitAnd",
            Operator::MatMult => "MatMult",
        };
        self.hash_tag(tag);
    }

    fn visit_unary_op(&mut self, op: &'a UnaryOp) {
        let tag = match op {
            UnaryOp::Invert => "Invert",
            UnaryOp::Not => "Not",
            UnaryOp::UAdd => "UAdd",
            UnaryOp::USub => "USub",
        };
        self.hash_tag(tag);
    }

    fn visit_cmp_op(&mut self, op: &'a CmpOp) {
        let tag = match op {
            CmpOp::Eq => "Eq",
            CmpOp::NotEq => "NotEq",
            CmpOp::Lt => "Lt",
            CmpOp::LtE => "LtE",
            CmpOp::Gt => "Gt",
            CmpOp::GtE => "GtE",
            CmpOp::Is => "Is",
            CmpOp::IsNot => "IsNot",
            CmpOp::In => "In",
            CmpOp::NotIn => "NotIn",
        };
        self.hash_tag(tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: hash a full single-line or multi-line Python snippet (lines 1..=N).
    fn hash_snippet(code: &str) -> u64 {
        let line_count = code.lines().count().max(1);
        hash_block(code, 1, line_count).expect("hash_block failed")
    }

    // ---- Phase 1 Exit Criteria Tests ----

    #[test]
    fn identical_structure_different_names_same_hash() {
        // `a = 1 + 2` and `x = 10 + 20` must produce the same hash.
        let h1 = hash_snippet("a = 1 + 2");
        let h2 = hash_snippet("x = 10 + 20");
        assert_eq!(h1, h2, "Structurally equivalent snippets must hash equal");
    }

    #[test]
    fn different_structure_different_hash() {
        // `a = 1 + 2` vs `a = 1 - 2` differ in operator.
        let h1 = hash_snippet("a = 1 + 2");
        let h2 = hash_snippet("a = 1 - 2");
        assert_ne!(h1, h2, "Different operators must produce different hashes");
    }

    #[test]
    fn multiline_blocks_same_structure() {
        let block_a = "x = 1\ny = x + 2\nz = y * 3";
        let block_b = "a = 100\nb = a + 200\nc = b * 300";
        let h1 = hash_snippet(block_a);
        let h2 = hash_snippet(block_b);
        assert_eq!(h1, h2, "Multi-line structurally equivalent blocks must match");
    }

    #[test]
    fn variable_reuse_pattern_matters() {
        // `a = 1; b = a + 2` (VAR_0 = CONST; VAR_1 = VAR_0 + CONST)
        // vs
        // `a = 1; b = b + 2` (VAR_0 = CONST; VAR_1 = VAR_1 + CONST)
        // These differ because the second expr refers to VAR_1 not VAR_0.
        let h1 = hash_snippet("a = 1\nb = a + 2");
        let h2 = hash_snippet("a = 1\nb = b + 2");
        assert_ne!(h1, h2, "Different variable-reference patterns must differ");
    }

    #[test]
    fn parse_error_does_not_crash() {
        let result = hash_block("def (broken", 1, 1);
        assert!(result.is_err(), "Parse errors should return Err, not panic");
    }

    #[test]
    fn no_statements_in_range_returns_error() {
        let result = hash_block("x = 1", 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn function_call_same_structure() {
        let h1 = hash_snippet("foo(x, y)");
        let h2 = hash_snippet("bar(a, b)");
        assert_eq!(h1, h2, "Function calls with same structure must match");
    }

    #[test]
    fn if_statement_same_structure() {
        let a = "if x > 0:\n    y = x + 1";
        let b = "if a > 0:\n    b = a + 1";
        let h1 = hash_snippet(a);
        let h2 = hash_snippet(b);
        assert_eq!(h1, h2, "If statements with same structure must match");
    }

    #[test]
    fn for_loop_same_structure() {
        let a = "for i in items:\n    print(i)";
        let b = "for x in data:\n    print(x)";
        let h1 = hash_snippet(a);
        let h2 = hash_snippet(b);
        assert_eq!(h1, h2, "For loops with same structure must match");
    }

    #[test]
    fn line_range_selection() {
        let code = "a = 1\nb = 2\nc = 3";
        // Lines 1-1 = just `a = 1`
        let h_line1 = hash_block(code, 1, 1).unwrap();
        // Lines 2-2 = just `b = 2`
        let h_line2 = hash_block(code, 2, 2).unwrap();
        // Both are `VAR = CONST` so they should match.
        assert_eq!(h_line1, h_line2, "Single assignment lines have same structure");

        // Lines 1-2 should differ from 1-1 (two statements vs one).
        let h_lines_1_2 = hash_block(code, 1, 2).unwrap();
        assert_ne!(h_line1, h_lines_1_2, "Different statement counts must differ");
    }
}
