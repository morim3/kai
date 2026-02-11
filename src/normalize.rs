use std::hash::{Hash, Hasher};

use anyhow::{Result, bail};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{BoolOp, CmpOp, Expr, ExprContext, Operator, Stmt, UnaryOp};
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
    source.as_bytes()[..offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Get the leading whitespace (indentation) of the line containing `offset`.
pub fn indent_at_offset(source: &str, offset: usize) -> String {
    let line_start = source[..offset].rfind('\n').map_or(0, |p| p + 1);
    source[line_start..offset].to_string()
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

/// Match an enum value to its variant name and hash it as a tag string.
macro_rules! hash_enum_tag {
    ($self:expr, $val:expr, $( $variant:pat => $tag:expr ),+ $(,)?) => {{
        let tag = match $val {
            $( $variant => $tag, )+
        };
        $self.hash_tag(tag);
    }};
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
        hash_enum_tag!(self, ctx,
            ExprContext::Load => "Load",
            ExprContext::Store => "Store",
            ExprContext::Del => "Del",
            ExprContext::Invalid => "Invalid",
        );
    }

    fn visit_bool_op(&mut self, op: &'a BoolOp) {
        hash_enum_tag!(self, op, BoolOp::And => "And", BoolOp::Or => "Or");
    }

    fn visit_operator(&mut self, op: &'a Operator) {
        hash_enum_tag!(self, op,
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
        );
    }

    fn visit_unary_op(&mut self, op: &'a UnaryOp) {
        hash_enum_tag!(self, op,
            UnaryOp::Invert => "Invert",
            UnaryOp::Not => "Not",
            UnaryOp::UAdd => "UAdd",
            UnaryOp::USub => "USub",
        );
    }

    fn visit_cmp_op(&mut self, op: &'a CmpOp) {
        hash_enum_tag!(self, op,
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
        );
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

    /// Parameterized test: pairs of structurally equivalent snippets must hash equal.
    #[test]
    fn structurally_equivalent_pairs_hash_equal() {
        let cases: &[(&str, &str, &str)] = &[
            ("a = 1 + 2", "x = 10 + 20", "simple assignment"),
            (
                "x = 1\ny = x + 2\nz = y * 3",
                "a = 100\nb = a + 200\nc = b * 300",
                "multi-line block",
            ),
            ("foo(x, y)", "bar(a, b)", "function call"),
            (
                "if x > 0:\n    y = x + 1",
                "if a > 0:\n    b = a + 1",
                "if statement",
            ),
            (
                "for i in items:\n    print(i)",
                "for x in data:\n    print(x)",
                "for loop",
            ),
        ];
        for (a, b, label) in cases {
            assert_eq!(
                hash_snippet(a),
                hash_snippet(b),
                "{label}: structurally equivalent snippets must hash equal"
            );
        }
    }

    /// Parameterized test: structurally different snippets must hash differently.
    #[test]
    fn structurally_different_pairs_hash_differ() {
        let cases: &[(&str, &str, &str)] = &[
            ("a = 1 + 2", "a = 1 - 2", "different operator"),
            (
                "a = 1\nb = a + 2",
                "a = 1\nb = b + 2",
                "different variable-reference pattern",
            ),
        ];
        for (a, b, label) in cases {
            assert_ne!(
                hash_snippet(a),
                hash_snippet(b),
                "{label}: structurally different snippets must hash differently"
            );
        }
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
    fn line_range_selection() {
        let code = "a = 1\nb = 2\nc = 3";
        let h_line1 = hash_block(code, 1, 1).unwrap();
        let h_line2 = hash_block(code, 2, 2).unwrap();
        assert_eq!(
            h_line1, h_line2,
            "Single assignment lines have same structure"
        );

        let h_lines_1_2 = hash_block(code, 1, 2).unwrap();
        assert_ne!(
            h_line1, h_lines_1_2,
            "Different statement counts must differ"
        );
    }
}
