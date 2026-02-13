use anyhow::{Result, bail};
use ruff_python_ast::{
    Comprehension, Expr, FStringPart, InterpolatedStringElement, Parameter, ParameterWithDefault,
    Pattern, Stmt,
};
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
        (Stmt::FunctionDef(a), Stmt::FunctionDef(b)) => {
            if a.name.as_str() != b.name.as_str() {
                out.push(Divergence::Name(a.name.to_string(), b.name.to_string()));
            }
            for (da, db) in a.decorator_list.iter().zip(b.decorator_list.iter()) {
                diff_exprs(&da.expression, &db.expression, sa, sb, out)?;
            }
            diff_parameters(&a.parameters, &b.parameters, sa, sb, out)?;
            if let (Some(ra), Some(rb)) = (&a.returns, &b.returns) {
                diff_exprs(ra, rb, sa, sb, out)?;
            }
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
        }
        (Stmt::ClassDef(a), Stmt::ClassDef(b)) => {
            if a.name.as_str() != b.name.as_str() {
                out.push(Divergence::Name(a.name.to_string(), b.name.to_string()));
            }
            for (da, db) in a.decorator_list.iter().zip(b.decorator_list.iter()) {
                diff_exprs(&da.expression, &db.expression, sa, sb, out)?;
            }
            if let (Some(args_a), Some(args_b)) = (&a.arguments, &b.arguments) {
                diff_expr_slices(&args_a.args, &args_b.args, sa, sb, out)?;
                for (ka, kb) in args_a.keywords.iter().zip(args_b.keywords.iter()) {
                    diff_exprs(&ka.value, &kb.value, sa, sb, out)?;
                }
            }
            diff_stmt_bodies(&a.body, &b.body, sa, sb, out)?;
        }
        (Stmt::Match(a), Stmt::Match(b)) => {
            diff_exprs(&a.subject, &b.subject, sa, sb, out)?;
            for (ca, cb) in a.cases.iter().zip(b.cases.iter()) {
                diff_patterns(&ca.pattern, &cb.pattern, sa, sb, out)?;
                if let (Some(ga), Some(gb)) = (&ca.guard, &cb.guard) {
                    diff_exprs(ga, gb, sa, sb, out)?;
                }
                diff_stmt_bodies(&ca.body, &cb.body, sa, sb, out)?;
            }
        }
        (Stmt::TypeAlias(a), Stmt::TypeAlias(b)) => {
            diff_exprs(&a.name, &b.name, sa, sb, out)?;
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
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

fn diff_exprs(a: &Expr, b: &Expr, sa: &str, sb: &str, out: &mut Vec<Divergence>) -> Result<()> {
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
            for (ka, kb) in a.arguments.keywords.iter().zip(b.arguments.keywords.iter()) {
                diff_exprs(&ka.value, &kb.value, sa, sb, out)?;
            }
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
        (Expr::Lambda(a), Expr::Lambda(b)) => {
            if let (Some(pa), Some(pb)) = (&a.parameters, &b.parameters) {
                diff_parameters(pa, pb, sa, sb, out)?;
            }
            diff_exprs(&a.body, &b.body, sa, sb, out)?;
        }
        (Expr::ListComp(a), Expr::ListComp(b)) => {
            diff_exprs(&a.elt, &b.elt, sa, sb, out)?;
            diff_comprehensions(&a.generators, &b.generators, sa, sb, out)?;
        }
        (Expr::SetComp(a), Expr::SetComp(b)) => {
            diff_exprs(&a.elt, &b.elt, sa, sb, out)?;
            diff_comprehensions(&a.generators, &b.generators, sa, sb, out)?;
        }
        (Expr::DictComp(a), Expr::DictComp(b)) => {
            diff_exprs(&a.key, &b.key, sa, sb, out)?;
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
            diff_comprehensions(&a.generators, &b.generators, sa, sb, out)?;
        }
        (Expr::Generator(a), Expr::Generator(b)) => {
            diff_exprs(&a.elt, &b.elt, sa, sb, out)?;
            diff_comprehensions(&a.generators, &b.generators, sa, sb, out)?;
        }
        (Expr::FString(a), Expr::FString(b)) => {
            for (pa, pb) in a.value.iter().zip(b.value.iter()) {
                match (pa, pb) {
                    (FStringPart::FString(fa), FStringPart::FString(fb)) => {
                        diff_interpolated_elements(&fa.elements, &fb.elements, sa, sb, out)?;
                    }
                    (FStringPart::Literal(la), FStringPart::Literal(lb)) => {
                        if la.value != lb.value {
                            out.push(Divergence::Literal(
                                quote_fstring_segment(&la.value),
                                quote_fstring_segment(&lb.value),
                            ));
                        }
                    }
                    _ => bail!("mismatched f-string parts in structurally identical blocks"),
                }
            }
        }
        (Expr::TString(a), Expr::TString(b)) => {
            for (ta, tb) in a.value.iter().zip(b.value.iter()) {
                diff_interpolated_elements(&ta.elements, &tb.elements, sa, sb, out)?;
            }
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

/// Diff two comprehension generator lists (used by ListComp, SetComp, DictComp, Generator).
fn diff_comprehensions(
    a: &[Comprehension],
    b: &[Comprehension],
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    for (ca, cb) in a.iter().zip(b.iter()) {
        diff_exprs(&ca.target, &cb.target, sa, sb, out)?;
        diff_exprs(&ca.iter, &cb.iter, sa, sb, out)?;
        diff_expr_slices(&ca.ifs, &cb.ifs, sa, sb, out)?;
    }
    Ok(())
}

/// Diff two interpolated string element sequences (used by FString and TString).
fn diff_interpolated_elements(
    a: &[InterpolatedStringElement],
    b: &[InterpolatedStringElement],
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    for (ea, eb) in a.iter().zip(b.iter()) {
        match (ea, eb) {
            (
                InterpolatedStringElement::Interpolation(ia),
                InterpolatedStringElement::Interpolation(ib),
            ) => {
                diff_exprs(&ia.expression, &ib.expression, sa, sb, out)?;
                if let (Some(fa), Some(fb)) = (&ia.format_spec, &ib.format_spec) {
                    diff_interpolated_elements(&fa.elements, &fb.elements, sa, sb, out)?;
                }
            }
            (InterpolatedStringElement::Literal(la), InterpolatedStringElement::Literal(lb)) => {
                if la.value != lb.value {
                    out.push(Divergence::Literal(
                        quote_fstring_segment(&la.value),
                        quote_fstring_segment(&lb.value),
                    ));
                }
            }
            _ => bail!("mismatched interpolated string elements in structurally identical blocks"),
        }
    }
    Ok(())
}

/// Quote an f-string literal segment value as a Python string literal.
///
/// Used for both divergence values (call-site arguments) and
/// `NodePosition::override_text` (rename-map lookup key).
pub fn quote_fstring_segment(raw: &str) -> String {
    let escaped = raw
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// Diff parameter default values and names across two `Parameters` structs.
fn diff_parameters(
    a: &ruff_python_ast::Parameters,
    b: &ruff_python_ast::Parameters,
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    let diff_param_with_defaults = |a: &[ParameterWithDefault],
                                    b: &[ParameterWithDefault],
                                    out: &mut Vec<Divergence>|
     -> Result<()> {
        for (pa, pb) in a.iter().zip(b.iter()) {
            diff_param_names(&pa.parameter, &pb.parameter, out);
            if let (Some(da), Some(db)) = (&pa.default, &pb.default) {
                diff_exprs(da, db, sa, sb, out)?;
            }
        }
        Ok(())
    };

    diff_param_with_defaults(&a.posonlyargs, &b.posonlyargs, out)?;
    diff_param_with_defaults(&a.args, &b.args, out)?;
    if let (Some(va), Some(vb)) = (&a.vararg, &b.vararg) {
        diff_param_names(va, vb, out);
    }
    diff_param_with_defaults(&a.kwonlyargs, &b.kwonlyargs, out)?;
    if let (Some(ka), Some(kb)) = (&a.kwarg, &b.kwarg) {
        diff_param_names(ka, kb, out);
    }
    Ok(())
}

/// Emit a Name divergence if two parameters have different names.
fn diff_param_names(a: &Parameter, b: &Parameter, out: &mut Vec<Divergence>) {
    if a.name.as_str() != b.name.as_str() {
        out.push(Divergence::Name(a.name.to_string(), b.name.to_string()));
    }
}

/// Recursively diff two `Pattern` nodes (used by Match statements).
fn diff_patterns(
    a: &Pattern,
    b: &Pattern,
    sa: &str,
    sb: &str,
    out: &mut Vec<Divergence>,
) -> Result<()> {
    match (a, b) {
        (Pattern::MatchValue(a), Pattern::MatchValue(b)) => {
            // MatchValue divergences are prevented at the hash stage:
            // different literal values produce different structural hashes,
            // so blocks with different case values never match.
            diff_exprs(&a.value, &b.value, sa, sb, out)?;
        }
        (Pattern::MatchSingleton(_), Pattern::MatchSingleton(_)) => {
            // None / True / False — no sub-expressions to diff
        }
        (Pattern::MatchSequence(a), Pattern::MatchSequence(b)) => {
            for (pa, pb) in a.patterns.iter().zip(b.patterns.iter()) {
                diff_patterns(pa, pb, sa, sb, out)?;
            }
        }
        (Pattern::MatchMapping(a), Pattern::MatchMapping(b)) => {
            // MatchMapping key divergences are prevented at the hash stage.
            diff_expr_slices(&a.keys, &b.keys, sa, sb, out)?;
            for (pa, pb) in a.patterns.iter().zip(b.patterns.iter()) {
                diff_patterns(pa, pb, sa, sb, out)?;
            }
            if let (Some(ra), Some(rb)) = (&a.rest, &b.rest)
                && ra.as_str() != rb.as_str()
            {
                out.push(Divergence::Name(ra.to_string(), rb.to_string()));
            }
        }
        (Pattern::MatchClass(a), Pattern::MatchClass(b)) => {
            diff_exprs(&a.cls, &b.cls, sa, sb, out)?;
            for (pa, pb) in a.arguments.patterns.iter().zip(b.arguments.patterns.iter()) {
                diff_patterns(pa, pb, sa, sb, out)?;
            }
            for (ka, kb) in a.arguments.keywords.iter().zip(b.arguments.keywords.iter()) {
                diff_patterns(&ka.pattern, &kb.pattern, sa, sb, out)?;
            }
        }
        (Pattern::MatchStar(a), Pattern::MatchStar(b)) => {
            if let (Some(na), Some(nb)) = (&a.name, &b.name)
                && na.as_str() != nb.as_str()
            {
                out.push(Divergence::Name(na.to_string(), nb.to_string()));
            }
        }
        (Pattern::MatchAs(a), Pattern::MatchAs(b)) => {
            if let (Some(pa), Some(pb)) = (&a.pattern, &b.pattern) {
                diff_patterns(pa, pb, sa, sb, out)?;
            }
            if let (Some(na), Some(nb)) = (&a.name, &b.name)
                && na.as_str() != nb.as_str()
            {
                out.push(Divergence::Name(na.to_string(), nb.to_string()));
            }
        }
        (Pattern::MatchOr(a), Pattern::MatchOr(b)) => {
            for (pa, pb) in a.patterns.iter().zip(b.patterns.iter()) {
                diff_patterns(pa, pb, sa, sb, out)?;
            }
        }
        _ => {
            bail!("mismatched pattern types in structurally identical blocks");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_stmts;

    /// Shorthand constructors for test readability.
    fn n(a: &str, b: &str) -> Divergence {
        Divergence::Name(a.into(), b.into())
    }
    fn l(a: &str, b: &str) -> Divergence {
        Divergence::Literal(a.into(), b.into())
    }

    /// Parse two sources and return the divergence list.
    fn divs(src_a: &str, src_b: &str) -> Vec<Divergence> {
        let a = parse_stmts(src_a);
        let b = parse_stmts(src_b);
        extract_divergences(&a, &b, src_a, src_b).unwrap()
    }

    // --- Basic divergence types ---

    #[test]
    fn name_divergence() {
        assert_eq!(
            divs("x = a + 1", "y = b + 1"),
            vec![n("a", "b"), n("x", "y")]
        );
    }

    #[test]
    fn literal_divergence() {
        assert_eq!(
            divs("x = 1 + 2", "x = 100 + 200"),
            vec![l("1", "100"), l("2", "200")]
        );
    }

    #[test]
    fn no_divergence_for_identical_code() {
        assert!(divs("x = 1 + 2", "x = 1 + 2").is_empty());
    }

    #[test]
    fn mixed_name_and_literal() {
        assert_eq!(
            divs("result = x + 10", "output = y + 20"),
            vec![n("x", "y"), l("10", "20"), n("result", "output")]
        );
    }

    // --- Compound statements ---

    #[test]
    fn if_body() {
        assert_eq!(
            divs("if x > 0:\n    a = 1", "if y > 0:\n    b = 2"),
            vec![n("x", "y"), l("1", "2"), n("a", "b")]
        );
    }

    #[test]
    fn for_loop() {
        assert_eq!(
            divs(
                "for i in items:\n    x = i + 1",
                "for j in data:\n    y = j + 2"
            ),
            vec![
                n("items", "data"),
                n("i", "j"),
                n("i", "j"),
                l("1", "2"),
                n("x", "y")
            ]
        );
    }

    #[test]
    fn while_loop() {
        assert_eq!(
            divs("while a < 10:\n    a += 1", "while b < 20:\n    b += 1"),
            vec![n("a", "b"), l("10", "20"), n("a", "b")]
        );
    }

    #[test]
    fn return_statement() {
        assert_eq!(
            divs("return x + 1", "return y + 2"),
            vec![n("x", "y"), l("1", "2")]
        );
    }

    #[test]
    fn with_statement() {
        assert_eq!(
            divs(
                "with open(file_a) as f:\n    data = f.read()",
                "with open(file_b) as g:\n    data = g.read()"
            ),
            vec![n("file_a", "file_b"), n("f", "g"), n("f", "g")]
        );
    }

    #[test]
    fn try_statement() {
        assert_eq!(
            divs(
                "try:\n    x = func_a()\nexcept Exception as e:\n    handle(e)",
                "try:\n    y = func_b()\nexcept Exception as e:\n    handle(e)"
            ),
            vec![n("func_a", "func_b"), n("x", "y")]
        );
    }

    #[test]
    fn assert_statement() {
        assert_eq!(
            divs(
                "assert x > 0, \"x must be positive\"",
                "assert y > 0, \"y must be positive\""
            ),
            vec![
                n("x", "y"),
                l("\"x must be positive\"", "\"y must be positive\"")
            ]
        );
    }

    #[test]
    fn raise_statement() {
        assert_eq!(
            divs("raise ValueError(msg_a)", "raise ValueError(msg_b)"),
            vec![n("msg_a", "msg_b")]
        );
    }

    #[test]
    fn match_safe_divergences() {
        // Same pattern values, divergences only in subject and body — safe.
        assert_eq!(
            divs(
                "match cmd_a:\n    case 1:\n        x = 10",
                "match cmd_b:\n    case 1:\n        y = 20"
            ),
            vec![n("cmd_a", "cmd_b"), l("10", "20"), n("x", "y")]
        );
    }

    #[test]
    fn match_as_pattern() {
        assert_eq!(
            divs(
                "match val:\n    case x as result_a:\n        pass",
                "match val:\n    case y as result_b:\n        pass"
            ),
            vec![n("x", "y"), n("result_a", "result_b")]
        );
    }

    // --- Expressions ---

    #[test]
    fn call_keyword_args() {
        assert_eq!(
            divs("func(key=value_a)", "func(key=value_b)"),
            vec![n("value_a", "value_b")]
        );
    }

    #[test]
    fn list_comprehension() {
        assert_eq!(
            divs(
                "result = [x + 1 for x in items]",
                "result = [y + 2 for y in data]"
            ),
            vec![n("x", "y"), l("1", "2"), n("x", "y"), n("items", "data")]
        );
    }

    #[test]
    fn dict_comprehension() {
        assert_eq!(
            divs(
                "d = {k: v for k, v in pairs_a}",
                "d = {k: v for k, v in pairs_b}"
            ),
            vec![n("pairs_a", "pairs_b")]
        );
    }

    #[test]
    fn generator_expression() {
        assert_eq!(
            divs(
                "s = sum(x * 2 for x in items)",
                "s = sum(y * 3 for y in data)"
            ),
            vec![n("x", "y"), l("2", "3"), n("x", "y"), n("items", "data")]
        );
    }

    #[test]
    fn comprehension_with_if() {
        assert_eq!(
            divs("[x for x in items if x > 0]", "[y for y in data if y > 0]"),
            vec![n("x", "y"), n("x", "y"), n("items", "data"), n("x", "y")]
        );
    }

    #[test]
    fn nested_comprehension() {
        assert_eq!(
            divs(
                "result = [x + y for x in items_a for y in items_b]",
                "result = [a + b for a in data_a for b in data_b]"
            ),
            vec![
                n("x", "a"),
                n("y", "b"),
                n("x", "a"),
                n("items_a", "data_a"),
                n("y", "b"),
                n("items_b", "data_b"),
            ]
        );
    }

    #[test]
    fn fstring() {
        assert_eq!(
            divs("s = f\"hello {name}\"", "s = f\"hello {user}\""),
            vec![n("name", "user")]
        );
    }

    #[test]
    fn fstring_multiple_exprs() {
        assert_eq!(
            divs("s = f\"{a} and {b}\"", "s = f\"{x} and {y}\""),
            vec![n("a", "x"), n("b", "y")]
        );
    }

    #[test]
    fn fstring_literal_segment_divergence() {
        // Different literal segments → per-segment Literal divergences (quoted).
        assert_eq!(
            divs(
                "s = f\"Pending: {order_id}\"",
                "s = f\"Shipped: {order_id}\""
            ),
            vec![l("\"Pending: \"", "\"Shipped: \"")]
        );
    }

    #[test]
    fn fstring_literal_and_expr_divergence() {
        // Both literal and expression differ → separate divergences.
        assert_eq!(
            divs("s = f\"Pending: {order_a}\"", "s = f\"Shipped: {order_b}\""),
            vec![l("\"Pending: \"", "\"Shipped: \""), n("order_a", "order_b")]
        );
    }

    #[test]
    fn lambda() {
        assert_eq!(
            divs("f = lambda x: x + 1", "f = lambda y: y + 2"),
            vec![n("x", "y"), n("x", "y"), l("1", "2")]
        );
    }

    #[test]
    fn lambda_with_default() {
        assert_eq!(
            divs("f = lambda x, y=10: x + y", "f = lambda a, b=20: a + b"),
            vec![
                n("x", "a"),
                n("y", "b"),
                l("10", "20"),
                n("x", "a"),
                n("y", "b")
            ]
        );
    }

    // --- Nested definitions ---

    #[test]
    fn nested_function_def() {
        assert_eq!(
            divs(
                "def helper_a(x):\n    return x + 1",
                "def helper_b(y):\n    return y + 2"
            ),
            vec![
                n("helper_a", "helper_b"),
                n("x", "y"),
                n("x", "y"),
                l("1", "2")
            ]
        );
    }

    #[test]
    fn function_def_body_only() {
        assert_eq!(
            divs(
                "def handler(event):\n    process(event, config_a)",
                "def handler(event):\n    process(event, config_b)"
            ),
            vec![n("config_a", "config_b")]
        );
    }

    #[test]
    fn class_def() {
        assert_eq!(
            divs(
                "class ViewA(Base):\n    name = \"a\"",
                "class ViewB(Base):\n    name = \"b\""
            ),
            vec![n("ViewA", "ViewB"), l("\"a\"", "\"b\"")]
        );
    }
}
