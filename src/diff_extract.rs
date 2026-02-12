use anyhow::{Result, bail};
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
///
/// Returns an error if the blocks contain statement or expression types whose
/// divergence extraction is not yet implemented.
pub fn extract_divergences(
    block_a: &[Stmt],
    block_b: &[Stmt],
    source_a: &str,
    source_b: &str,
) -> Result<Vec<Divergence>> {
    let mut divergences = Vec::new();
    for (a, b) in block_a.iter().zip(block_b.iter()) {
        diff_stmts(a, b, source_a, source_b, &mut divergences)?;
    }
    Ok(divergences)
}

fn diff_stmts(a: &Stmt, b: &Stmt, sa: &str, sb: &str, out: &mut Vec<Divergence>) -> Result<()> {
    match (a, b) {
        (Stmt::Assign(a), Stmt::Assign(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
            for (ta, tb) in a.targets.iter().zip(b.targets.iter()) {
                diff_exprs(ta, tb, sa, sb, out)?;
            }
        }
        (Stmt::AugAssign(a), Stmt::AugAssign(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
            diff_exprs(&a.target, &b.target, sa, sb, out)?;
        }
        (Stmt::AnnAssign(a), Stmt::AnnAssign(b)) => {
            diff_exprs(&a.target, &b.target, sa, sb, out)?;
            diff_exprs(&a.annotation, &b.annotation, sa, sb, out)?;
            if let (Some(va), Some(vb)) = (&a.value, &b.value) {
                diff_exprs(va, vb, sa, sb, out)?;
            }
        }
        (Stmt::Expr(a), Stmt::Expr(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Stmt::Return(a), Stmt::Return(b)) => {
            if let (Some(va), Some(vb)) = (&a.value, &b.value) {
                diff_exprs(va, vb, sa, sb, out)?;
            }
        }
        (Stmt::Delete(a), Stmt::Delete(b)) => {
            for (ta, tb) in a.targets.iter().zip(b.targets.iter()) {
                diff_exprs(ta, tb, sa, sb, out)?;
            }
        }
        (Stmt::If(a), Stmt::If(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out)?;
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
            for (ea, eb) in a.elif_else_clauses.iter().zip(b.elif_else_clauses.iter()) {
                if let (Some(ta), Some(tb)) = (&ea.test, &eb.test) {
                    diff_exprs(ta, tb, sa, sb, out)?;
                }
                diff_stmt_bodies(&ea.body, &eb.body, sa, sb, out)?;
            }
        }
        (Stmt::For(a), Stmt::For(b)) => {
            diff_exprs(&a.iter, &b.iter, sa, sb, out)?;
            diff_exprs(&a.target, &b.target, sa, sb, out)?;
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
            diff_stmt_bodies(&a.orelse, &b.orelse, sa, sb, out)?;
        }
        (Stmt::While(a), Stmt::While(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out)?;
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
            diff_stmt_bodies(&a.orelse, &b.orelse, sa, sb, out)?;
        }
        (Stmt::With(a), Stmt::With(b)) => {
            for (ia, ib) in a.items.iter().zip(b.items.iter()) {
                diff_exprs(&ia.context_expr, &ib.context_expr, sa, sb, out)?;
                if let (Some(va), Some(vb)) = (&ia.optional_vars, &ib.optional_vars) {
                    diff_exprs(va, vb, sa, sb, out)?;
                }
            }
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
        }
        (Stmt::Raise(a), Stmt::Raise(b)) => {
            if let (Some(ea), Some(eb)) = (&a.exc, &b.exc) {
                diff_exprs(ea, eb, sa, sb, out)?;
            }
            if let (Some(ca), Some(cb)) = (&a.cause, &b.cause) {
                diff_exprs(ca, cb, sa, sb, out)?;
            }
        }
        (Stmt::Try(a), Stmt::Try(b)) => {
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
            for (ha, hb) in a.handlers.iter().zip(b.handlers.iter()) {
                let (
                    ruff_python_ast::ExceptHandler::ExceptHandler(ha),
                    ruff_python_ast::ExceptHandler::ExceptHandler(hb),
                ) = (ha, hb);
                if let (Some(ta), Some(tb)) = (&ha.type_, &hb.type_) {
                    diff_exprs(ta, tb, sa, sb, out)?;
                }
                diff_stmt_bodies(&ha.body, &hb.body, sa, sb, out)?;
            }
            diff_stmt_bodies(&a.orelse, &b.orelse, sa, sb, out)?;
            diff_stmt_bodies(&a.finalbody, &b.finalbody, sa, sb, out)?;
        }
        (Stmt::Assert(a), Stmt::Assert(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out)?;
            if let (Some(ma), Some(mb)) = (&a.msg, &b.msg) {
                diff_exprs(ma, mb, sa, sb, out)?;
            }
        }
        // No sub-expressions to compare.
        (Stmt::Pass(_), Stmt::Pass(_))
        | (Stmt::Break(_), Stmt::Break(_))
        | (Stmt::Continue(_), Stmt::Continue(_))
        | (Stmt::Import(_), Stmt::Import(_))
        | (Stmt::ImportFrom(_), Stmt::ImportFrom(_))
        | (Stmt::Global(_), Stmt::Global(_))
        | (Stmt::Nonlocal(_), Stmt::Nonlocal(_)) => {}
        // Not yet implemented.
        (Stmt::FunctionDef(_), Stmt::FunctionDef(_)) => {
            bail!("divergence extraction not implemented for nested function definitions");
        }
        (Stmt::ClassDef(_), Stmt::ClassDef(_)) => {
            bail!("divergence extraction not implemented for nested class definitions");
        }
        (Stmt::Match(_), Stmt::Match(_)) => {
            bail!("divergence extraction not implemented for match statements");
        }
        (Stmt::TypeAlias(_), Stmt::TypeAlias(_)) => {
            bail!("divergence extraction not implemented for type alias statements");
        }
        (Stmt::IpyEscapeCommand(_), Stmt::IpyEscapeCommand(_)) => {
            bail!("divergence extraction not implemented for IPython escape commands");
        }
        // Mismatched variants — should not happen with structurally identical blocks.
        _ => {
            bail!("mismatched statement types in structurally identical blocks");
        }
    }
    Ok(())
}

fn diff_stmt_bodies(
    a: &[Stmt],
    b: &[Stmt],
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    for (stmt_a, stmt_b) in a.iter().zip(b.iter()) {
        diff_stmts(stmt_a, stmt_b, sa, sb, out)?;
    }
    Ok(())
}

fn diff_expr_slices(
    a: &[Expr],
    b: &[Expr],
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    for (ea, eb) in a.iter().zip(b.iter()) {
        diff_exprs(ea, eb, sa, sb, out)?;
    }
    Ok(())
}

fn diff_exprs(
    a: &Expr,
    b: &Expr,
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
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

        // Constant literals with no value to compare.
        (Expr::NoneLiteral(_), Expr::NoneLiteral(_))
        | (Expr::EllipsisLiteral(_), Expr::EllipsisLiteral(_)) => {}

        // Recurse into compound expressions.
        (Expr::BinOp(a), Expr::BinOp(b)) => {
            diff_exprs(&a.left, &b.left, sa, sb, out)?;
            diff_exprs(&a.right, &b.right, sa, sb, out)?;
        }
        (Expr::UnaryOp(a), Expr::UnaryOp(b)) => {
            diff_exprs(&a.operand, &b.operand, sa, sb, out)?;
        }
        (Expr::BoolOp(a), Expr::BoolOp(b)) => {
            diff_expr_slices(&a.values, &b.values, sa, sb, out)?;
        }
        (Expr::Named(a), Expr::Named(b)) => {
            diff_exprs(&a.target, &b.target, sa, sb, out)?;
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Expr::Call(a), Expr::Call(b)) => {
            diff_exprs(&a.func, &b.func, sa, sb, out)?;
            diff_expr_slices(&a.arguments.args, &b.arguments.args, sa, sb, out)?;
        }
        (Expr::Compare(a), Expr::Compare(b)) => {
            diff_exprs(&a.left, &b.left, sa, sb, out)?;
            diff_expr_slices(&a.comparators, &b.comparators, sa, sb, out)?;
        }
        (Expr::Attribute(a), Expr::Attribute(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Expr::Subscript(a), Expr::Subscript(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
            diff_exprs(&a.slice, &b.slice, sa, sb, out)?;
        }
        (Expr::Starred(a), Expr::Starred(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Expr::List(a), Expr::List(b)) => {
            diff_expr_slices(&a.elts, &b.elts, sa, sb, out)?;
        }
        (Expr::Tuple(a), Expr::Tuple(b)) => {
            diff_expr_slices(&a.elts, &b.elts, sa, sb, out)?;
        }
        (Expr::Set(a), Expr::Set(b)) => {
            diff_expr_slices(&a.elts, &b.elts, sa, sb, out)?;
        }
        (Expr::Dict(a), Expr::Dict(b)) => {
            for (ia, ib) in a.items.iter().zip(b.items.iter()) {
                if let (Some(ka), Some(kb)) = (&ia.key, &ib.key) {
                    diff_exprs(ka, kb, sa, sb, out)?;
                }
                diff_exprs(&ia.value, &ib.value, sa, sb, out)?;
            }
        }
        (Expr::Slice(a), Expr::Slice(b)) => {
            if let (Some(la), Some(lb)) = (&a.lower, &b.lower) {
                diff_exprs(la, lb, sa, sb, out)?;
            }
            if let (Some(ua), Some(ub)) = (&a.upper, &b.upper) {
                diff_exprs(ua, ub, sa, sb, out)?;
            }
            if let (Some(sa_), Some(sb_)) = (&a.step, &b.step) {
                diff_exprs(sa_, sb_, sa, sb, out)?;
            }
        }
        (Expr::If(a), Expr::If(b)) => {
            diff_exprs(&a.test, &b.test, sa, sb, out)?;
            diff_exprs(&a.body, &b.body, sa, sb, out)?;
            diff_exprs(&a.orelse, &b.orelse, sa, sb, out)?;
        }
        (Expr::Await(a), Expr::Await(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Expr::Yield(a), Expr::Yield(b)) => {
            if let (Some(va), Some(vb)) = (&a.value, &b.value) {
                diff_exprs(va, vb, sa, sb, out)?;
            }
        }
        (Expr::YieldFrom(a), Expr::YieldFrom(b)) => {
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        // Not yet implemented.
        (Expr::Lambda(_), Expr::Lambda(_)) => {
            bail!("divergence extraction not implemented for lambda expressions");
        }
        (Expr::ListComp(_), Expr::ListComp(_)) => {
            bail!("divergence extraction not implemented for list comprehensions");
        }
        (Expr::SetComp(_), Expr::SetComp(_)) => {
            bail!("divergence extraction not implemented for set comprehensions");
        }
        (Expr::DictComp(_), Expr::DictComp(_)) => {
            bail!("divergence extraction not implemented for dict comprehensions");
        }
        (Expr::Generator(_), Expr::Generator(_)) => {
            bail!("divergence extraction not implemented for generator expressions");
        }
        (Expr::FString(_), Expr::FString(_)) => {
            bail!("divergence extraction not implemented for f-string expressions");
        }
        (Expr::TString(_), Expr::TString(_)) => {
            bail!("divergence extraction not implemented for t-string expressions");
        }
        (Expr::IpyEscapeCommand(_), Expr::IpyEscapeCommand(_)) => {
            bail!("divergence extraction not implemented for IPython escape commands");
        }
        // Mismatched variants — should not happen with structurally identical blocks.
        _ => {
            bail!("mismatched expression types in structurally identical blocks");
        }
    }
    Ok(())
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
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
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
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert_eq!(divs.len(), 2);
        assert_eq!(divs[0], Divergence::Literal("1".into(), "100".into()));
        assert_eq!(divs[1], Divergence::Literal("2".into(), "200".into()));
    }

    #[test]
    fn no_divergence_for_identical_code() {
        let src = "x = 1 + 2";
        let a = parse_stmts(src);
        let b = parse_stmts(src);
        let divs = extract_divergences(&a, &b, src, src).unwrap();
        assert!(divs.is_empty());
    }

    #[test]
    fn mixed_name_and_literal_divergence() {
        let src_a = "result = x + 10";
        let src_b = "output = y + 20";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert_eq!(divs.len(), 3);
        assert_eq!(divs[0], Divergence::Name("x".into(), "y".into()));
        assert_eq!(divs[1], Divergence::Literal("10".into(), "20".into()));
        assert_eq!(divs[2], Divergence::Name("result".into(), "output".into()));
    }

    #[test]
    fn divergence_inside_if_body() {
        let src_a = "if x > 0:\n    a = 1";
        let src_b = "if y > 0:\n    b = 2";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        // test: x vs y, body: a=1 vs b=2 (literal 1 vs 2, name a vs b)
        assert_eq!(divs.len(), 3);
        assert_eq!(divs[0], Divergence::Name("x".into(), "y".into()));
        assert_eq!(divs[1], Divergence::Literal("1".into(), "2".into()));
        assert_eq!(divs[2], Divergence::Name("a".into(), "b".into()));
    }

    #[test]
    fn divergence_inside_for_loop() {
        let src_a = "for i in items:\n    x = i + 1";
        let src_b = "for j in data:\n    y = j + 2";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        // iter: items vs data, target: i vs j, body: i vs j + 1 vs 2, x vs y
        assert_eq!(divs.len(), 5);
        assert_eq!(divs[0], Divergence::Name("items".into(), "data".into()));
        assert_eq!(divs[1], Divergence::Name("i".into(), "j".into()));
        assert_eq!(divs[2], Divergence::Name("i".into(), "j".into()));
        assert_eq!(divs[3], Divergence::Literal("1".into(), "2".into()));
        assert_eq!(divs[4], Divergence::Name("x".into(), "y".into()));
    }

    #[test]
    fn divergence_inside_while_loop() {
        let src_a = "while a < 10:\n    a += 1";
        let src_b = "while b < 20:\n    b += 1";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        // test: a vs b, literal: 10 vs 20, body aug_assign: 1 vs 1 (same), a vs b
        assert_eq!(divs.len(), 3);
        assert_eq!(divs[0], Divergence::Name("a".into(), "b".into()));
        assert_eq!(divs[1], Divergence::Literal("10".into(), "20".into()));
        assert_eq!(divs[2], Divergence::Name("a".into(), "b".into()));
    }

    #[test]
    fn divergence_in_return_statement() {
        let src_a = "return x + 1";
        let src_b = "return y + 2";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert_eq!(divs.len(), 2);
        assert_eq!(divs[0], Divergence::Name("x".into(), "y".into()));
        assert_eq!(divs[1], Divergence::Literal("1".into(), "2".into()));
    }

    #[test]
    fn divergence_in_with_statement() {
        let src_a = "with open(file_a) as f:\n    data = f.read()";
        let src_b = "with open(file_b) as g:\n    data = g.read()";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert!(divs.iter().any(|d| *d == Divergence::Name("file_a".into(), "file_b".into())));
        assert!(divs.iter().any(|d| *d == Divergence::Name("f".into(), "g".into())));
    }

    #[test]
    fn divergence_in_try_statement() {
        let src_a = "try:\n    x = func_a()\nexcept Exception as e:\n    handle(e)";
        let src_b = "try:\n    y = func_b()\nexcept Exception as e:\n    handle(e)";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert!(divs.iter().any(|d| *d == Divergence::Name("func_a".into(), "func_b".into())));
        assert!(divs.iter().any(|d| *d == Divergence::Name("x".into(), "y".into())));
    }

    #[test]
    fn divergence_in_assert_statement() {
        let src_a = "assert x > 0, \"x must be positive\"";
        let src_b = "assert y > 0, \"y must be positive\"";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert!(divs.iter().any(|d| *d == Divergence::Name("x".into(), "y".into())));
    }

    #[test]
    fn divergence_in_raise_statement() {
        let src_a = "raise ValueError(msg_a)";
        let src_b = "raise ValueError(msg_b)";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let divs = extract_divergences(&a, &b, src_a, src_b).unwrap();
        assert_eq!(divs, vec![Divergence::Name("msg_a".into(), "msg_b".into())]);
    }

    #[test]
    fn not_implemented_for_match() {
        let src_a = "match x:\n    case 1:\n        pass";
        let src_b = "match y:\n    case 2:\n        pass";
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        let err = extract_divergences(&a, &b, src_a, src_b).unwrap_err();
        assert!(err.to_string().contains("not implemented"), "{err}");
    }
}
