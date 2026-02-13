use std::hash::{Hash, Hasher};

use ruff_python_ast::visitor::{
    Visitor, walk_comprehension, walk_expr, walk_interpolated_string_element, walk_keyword,
    walk_pattern, walk_stmt,
};
use ruff_python_ast::{
    BoolOp, CmpOp, Comprehension, Expr, ExprContext, InterpolatedStringElement, Keyword, Operator,
    Pattern, Stmt, UnaryOp,
};
use ruff_text_size::Ranged;
use rustc_hash::FxHasher;

/// Compute the structural hash of statements via an iterator of references.
fn hash_stmt_iter<'a>(stmts: impl Iterator<Item = &'a Stmt>, source: &str) -> u64 {
    let mut visitor = NormalizeVisitor::new(source);
    for stmt in stmts {
        visitor.visit_stmt(stmt);
    }
    visitor.finish()
}

/// Compute the structural hash of a slice of AST statements.
pub fn hash_stmts(stmts: &[Stmt], source: &str) -> u64 {
    hash_stmt_iter(stmts.iter(), source)
}

/// Compute the structural hash of a collection of statement references.
pub fn hash_stmt_refs(stmts: &[&Stmt], source: &str) -> u64 {
    hash_stmt_iter(stmts.iter().copied(), source)
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
#[allow(clippy::naive_bytecount)]
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
/// - Literals are replaced with a constant token (except inside match value patterns).
/// - The structure (node types, operators, nesting) is fully hashed.
struct NormalizeVisitor<'s> {
    hasher: FxHasher,
    /// Maps original variable names to sequential IDs (insertion order).
    var_map: Vec<String>,
    /// Source text for hashing literal values in match patterns.
    source: &'s str,
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

/// Normalized token for all literal types (numbers, strings, booleans, None, ellipsis).
const LITERAL_TOKEN: &str = "CONSTANT";

impl<'s> NormalizeVisitor<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            hasher: FxHasher::default(),
            var_map: Vec::new(),
            source,
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

    /// Hash the source text of an AST node by its range.
    fn hash_source_range(&mut self, node: &impl Ranged) {
        let r = node.range();
        self.hash_tag(&self.source[r.start().to_usize()..r.end().to_usize()]);
    }
}

impl<'a> Visitor<'a> for NormalizeVisitor<'_> {
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

        // Hash non-Expr fields that walk_stmt doesn't visit.
        match stmt {
            Stmt::For(f) => f.is_async.hash(&mut self.hasher),
            Stmt::With(w) => w.is_async.hash(&mut self.hasher),
            Stmt::Try(t) => t.is_star.hash(&mut self.hasher),
            Stmt::AnnAssign(a) => a.simple.hash(&mut self.hasher),
            Stmt::ImportFrom(i) => i.level.hash(&mut self.hasher),
            _ => {}
        }

        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            // Variable names: normalize to positional IDs.
            Expr::Name(name) => {
                self.hash_tag("Name");
                let var_id = self.var_id(&name.id);
                self.hash_usize(var_id);
                // Hash context (Load/Store/Del) so that `x = ...` and `... = x` differ.
                self.visit_expr_context(&name.ctx);
            }

            // All literals: hash as a single normalized token.
            Expr::NumberLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_) => {
                self.hash_tag(LITERAL_TOKEN);
            }

            // Attribute: hash the .attr Identifier (not visited by walk_expr).
            Expr::Attribute(attr) => {
                self.hash_tag("Attribute");
                self.hash_tag(attr.attr.as_str());
                walk_expr(self, expr);
            }

            // Tuple: hash parenthesized flag (not visited by walk_expr).
            Expr::Tuple(t) => {
                self.hash_tag("Tuple");
                t.parenthesized.hash(&mut self.hasher);
                walk_expr(self, expr);
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
                    Expr::Subscript(_) => "Subscript",
                    Expr::Starred(_) => "Starred",
                    Expr::List(_) => "List",
                    Expr::Slice(_) => "Slice",
                    Expr::IpyEscapeCommand(_) => "IpyEscapeCommand",
                    // Already handled above.
                    Expr::Name(_)
                    | Expr::Attribute(_)
                    | Expr::Tuple(_)
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

    fn visit_keyword(&mut self, keyword: &'a Keyword) {
        // Hash the keyword argument name (not visited by walk_keyword).
        if let Some(ref arg) = keyword.arg {
            self.hash_tag(arg.as_str());
        }
        walk_keyword(self, keyword);
    }

    fn visit_comprehension(&mut self, comprehension: &'a Comprehension) {
        // Hash is_async (not visited by walk_comprehension).
        comprehension.is_async.hash(&mut self.hasher);
        walk_comprehension(self, comprehension);
    }

    fn visit_interpolated_string_element(
        &mut self,
        element: &'a InterpolatedStringElement,
    ) {
        // Hash the conversion flag (!r, !s, !a) which walk_interpolated_string_element ignores.
        // Without this, `f"{x!r}"` and `f"{x!s}"` would hash identically.
        if let InterpolatedStringElement::Interpolation(interp) = element {
            (interp.conversion as i8).hash(&mut self.hasher);
        }
        walk_interpolated_string_element(self, element);
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        match pattern {
            // MatchValue: hash actual literal source text instead of normalizing.
            // `case 1:` and `case 2:` must produce different hashes because
            // parameterizing a value pattern (e.g., `case arg_0:`) turns it into
            // a capture pattern with completely different match semantics.
            Pattern::MatchValue(mv) => {
                self.hash_tag("MatchValue");
                self.hash_source_range(&*mv.value);
            }

            // MatchMapping keys: same issue — keys must be literals or dotted names.
            Pattern::MatchMapping(mm) => {
                self.hash_tag("MatchMapping");

                // Hash all keys (must be literals or dotted names).
                for key in &mm.keys {
                    self.hash_source_range(key);
                }

                // Walk the rest (value patterns, rest name) normally.
                for pat in &mm.patterns {
                    self.visit_pattern(pat);
                }
                if let Some(ref rest) = mm.rest {
                    self.hash_tag(rest.as_str());
                }
            }

            _ => {
                walk_pattern(self, pattern);
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
    use anyhow::{Result, bail};

    /// Extract the statements covering `start_line..=end_line` (1-based) from source.
    /// Returns a hash of the structurally-normalized AST for those statements.
    fn hash_block(source: &str, start_line: usize, end_line: usize) -> Result<u64> {
        if start_line == 0 || end_line == 0 || start_line > end_line {
            bail!("Invalid line range: {start_line}..={end_line}");
        }

        let parsed = crate::parse_python(source)?;

        let stmts = select_stmts(source, &parsed.syntax().body, start_line, end_line);
        if stmts.is_empty() {
            bail!("No statements found in range {start_line}..={end_line}");
        }

        Ok(hash_stmt_refs(&stmts, source))
    }

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
            (
                "match cmd_a:\n    case 1:\n        x = 10",
                "match cmd_b:\n    case 1:\n        y = 20",
                "match with same case value",
            ),
            (
                "s = f\"hello {name}\"",
                "s = f\"hello {user}\"",
                "fstring with same literal text",
            ),
            (
                "s = f\"Pending: {x}\"",
                "s = f\"Shipped: {x}\"",
                "fstring with different literal text (whole-expr divergence)",
            ),
            (
                "s = f\"{x!r}\"",
                "s = f\"{y!r}\"",
                "fstring with same conversion flag",
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
            (
                "s = f\"{x!r}\"",
                "s = f\"{x!s}\"",
                "fstring conversion flag !r vs !s",
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

    /// Non-Expr fields (Identifier, bool) that were previously ignored must
    /// produce different hashes.
    #[test]
    fn non_expr_fields_hash_differ() {
        let cases: &[(&str, &str, &str)] = &[
            ("obj.read()", "obj.write()", "Attribute .attr"),
            ("func(a=1)", "func(b=1)", "Keyword .arg"),
            (
                "for x in y:\n    pass",
                "async for x in y:\n    pass",
                "For is_async",
            ),
            (
                "with ctx():\n    pass",
                "async with ctx():\n    pass",
                "With is_async",
            ),
            (
                "try:\n    pass\nexcept E:\n    pass",
                "try:\n    pass\nexcept* E:\n    pass",
                "Try is_star",
            ),
            (
                "[x for x in y]",
                "[x async for x in y]",
                "Comprehension is_async",
            ),
            (
                "match x:\n    case 1:\n        pass",
                "match x:\n    case 2:\n        pass",
                "MatchValue literal",
            ),
            (
                "x: int = 1",
                "(x): int = 1",
                "AnnAssign simple",
            ),
            (
                "x = (a, b)",
                "x = a, b",
                "Tuple parenthesized",
            ),
            (
                "from . import x",
                "from .. import x",
                "ImportFrom level",
            ),
        ];
        for (a, b, label) in cases {
            assert_ne!(
                hash_snippet(a),
                hash_snippet(b),
                "{label}: non-Expr field must affect hash"
            );
        }
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

