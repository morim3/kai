use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::Ranged;

/// A difference between two structurally equivalent AST nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// Two Name nodes with different identifiers.
    /// (name_in_block_0, name_in_block_1)
    Name(String, String),
    /// Two literal nodes with different source text.
    /// (source_text_in_block_0, source_text_in_block_1)
    Literal(String, String),
}

/// Walk two structurally identical statement sequences in parallel,
/// collecting all positions where concrete values differ.
///
/// `source_a` and `source_b` are the full source texts (used to extract literal text).
/// Returns divergences in AST traversal order.
pub fn extract_divergences(
    block_a: &[Stmt],
    block_b: &[Stmt],
    source_a: &str,
    source_b: &str,
) -> Vec<Divergence> {
    let mut divergences = Vec::new();
    for (a, b) in block_a.iter().zip(block_b.iter()) {
        diff_stmts(a, b, source_a, source_b, &mut divergences);
    }
    divergences
}

fn diff_stmts(a: &Stmt, b: &Stmt, sa: &str, sb: &str, out: &mut Vec<Divergence>) {
    match (a, b) {
        (Stmt::Assign(a), Stmt::Assign(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out);
            for (ta, tb) in a.targets.iter().zip(b.targets.iter()) {
                diff_exprs(ta, tb, sa, sb, out);
            }
        }
        (Stmt::AugAssign(a), Stmt::AugAssign(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out);
            diff_exprs(&a.target, &b.target, sa, sb, out);
        }
        (Stmt::Expr(a), Stmt::Expr(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out);
        }
        (Stmt::Return(a), Stmt::Return(b)) => {
            if let (Some(va), Some(vb)) = (&a.value, &b.value) {
                diff_exprs(va, vb, sa, sb, out);
            }
        }
        (Stmt::If(a), Stmt::If(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out);
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out);
            for (ea, eb) in a.elif_else_clauses.iter().zip(b.elif_else_clauses.iter()) {
                if let (Some(ta), Some(tb)) = (&ea.test, &eb.test) {
                    diff_exprs(ta, tb, sa, sb, out);
                }
                diff_stmt_bodies(&ea.body, &eb.body, sa, sb, out);
            }
        }
        (Stmt::For(a), Stmt::For(b)) => {
            diff_exprs(&a.iter, &b.iter, sa, sb, out);
            diff_exprs(&a.target, &b.target, sa, sb, out);
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out);
        }
        (Stmt::While(a), Stmt::While(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out);
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out);
        }
        _ => {}
    }
}

fn diff_stmt_bodies(a: &[Stmt], b: &[Stmt], sa: &str, sb: &str, out: &mut Vec<Divergence>) {
    for (stmt_a, stmt_b) in a.iter().zip(b.iter()) {
        diff_stmts(stmt_a, stmt_b, sa, sb, out);
    }
}

fn diff_exprs(a: &Expr, b: &Expr, sa: &str, sb: &str, out: &mut Vec<Divergence>) {
    match (a, b) {
        (Expr::Name(a), Expr::Name(b)) => {
            if a.id != b.id {
                out.push(Divergence::Name(a.id.to_string(), b.id.to_string()));
            }
        }

        // Literals: use source text.
        (Expr::NumberLiteral(_), Expr::NumberLiteral(_))
        | (Expr::StringLiteral(_), Expr::StringLiteral(_))
        | (Expr::BytesLiteral(_), Expr::BytesLiteral(_))
        | (Expr::BooleanLiteral(_), Expr::BooleanLiteral(_)) => {
            let a_range = a.range();
            let b_range = b.range();
            let a_text = &sa[a_range.start().to_usize()..a_range.end().to_usize()];
            let b_text = &sb[b_range.start().to_usize()..b_range.end().to_usize()];
            if a_text != b_text {
                out.push(Divergence::Literal(a_text.to_string(), b_text.to_string()));
            }
        }

        // Recurse into compound expressions.
        (Expr::BinOp(a), Expr::BinOp(b)) => {
            diff_exprs(&a.left, &b.left, sa, sb, out);
            diff_exprs(&a.right, &b.right, sa, sb, out);
        }
        (Expr::UnaryOp(a), Expr::UnaryOp(b)) => {
            diff_exprs(&a.operand, &b.operand, sa, sb, out);
        }
        (Expr::Call(a), Expr::Call(b)) => {
            diff_exprs(&a.func, &b.func, sa, sb, out);
            for (aa, ab) in a.arguments.args.iter().zip(b.arguments.args.iter()) {
                diff_exprs(aa, ab, sa, sb, out);
            }
        }
        (Expr::Compare(a), Expr::Compare(b)) => {
            diff_exprs(&a.left, &b.left, sa, sb, out);
            for (ca, cb) in a.comparators.iter().zip(b.comparators.iter()) {
                diff_exprs(ca, cb, sa, sb, out);
            }
        }
        (Expr::Attribute(a), Expr::Attribute(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out);
        }
        (Expr::Subscript(a), Expr::Subscript(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out);
            diff_exprs(&a.slice, &b.slice, sa, sb, out);
        }
        (Expr::List(a), Expr::List(b)) => {
            for (ea, eb) in a.elts.iter().zip(b.elts.iter()) {
                diff_exprs(ea, eb, sa, sb, out);
            }
        }
        (Expr::Tuple(a), Expr::Tuple(b)) => {
            for (ea, eb) in a.elts.iter().zip(b.elts.iter()) {
                diff_exprs(ea, eb, sa, sb, out);
            }
        }
        (Expr::If(a), Expr::If(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out);
            diff_exprs(&a.body, &b.body, sa, sb, out);
            diff_exprs(&a.orelse, &b.orelse, sa, sb, out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_stmts;

    #[test]
    fn detects_name_divergence() {
        let src_a = "x = a + 1";
        let src_b = "y = b + 1";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b);
        assert_eq!(divs.len(), 2);
        assert_eq!(divs[0], Divergence::Name("a".into(), "b".into()));
        assert_eq!(divs[1], Divergence::Name("x".into(), "y".into()));
    }

    #[test]
    fn detects_literal_divergence() {
        let src_a = "x = 1 + 2";
        let src_b = "x = 100 + 200";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b);
        assert_eq!(divs.len(), 2);
        assert_eq!(divs[0], Divergence::Literal("1".into(), "100".into()));
        assert_eq!(divs[1], Divergence::Literal("2".into(), "200".into()));
    }

    #[test]
    fn no_divergence_for_identical_code() {
        let src = "x = 1 + 2";
        let a = parse_stmts(src);
        let b = parse_stmts(src);
        let divs = extract_divergences(&a, &b, src, src);
        assert!(divs.is_empty());
    }

    #[test]
    fn mixed_name_and_literal_divergence() {
        let src_a = "result = x + 10";
        let src_b = "output = y + 20";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b);
        assert_eq!(divs.len(), 3);
        assert_eq!(divs[0], Divergence::Name("x".into(), "y".into()));
        assert_eq!(divs[1], Divergence::Literal("10".into(), "20".into()));
        assert_eq!(divs[2], Divergence::Name("result".into(), "output".into()));
    }
}
