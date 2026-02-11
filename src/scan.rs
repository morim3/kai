use anyhow::{Result, bail};
use ruff_python_ast::Stmt;
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;

use crate::normalize::{hash_stmt_refs, hash_stmts, line_of_offset, select_stmts};

/// The kind of scope that contains matched blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    Function,
    Class,
}

/// Context about the scope where the extracted function should be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeContext {
    pub kind: ScopeKind,
    /// Byte offset of the first statement in the scope body (insertion point).
    pub body_start_offset: usize,
    /// Indentation string for code inside this scope (e.g. "    " for function body).
    pub indent: String,
    /// For Class scope: byte offset of the `class` statement (to insert function before it).
    pub class_def_offset: Option<usize>,
    /// For Class scope: indentation of the parent scope.
    pub parent_indent: Option<String>,
}

/// Find the innermost scope body containing the given line range.
///
/// Recursively drills into `FunctionDef` (including async) and `ClassDef`
/// to find the deepest body that fully contains `target_start..=target_end`.
///
/// Returns the body slice and a `ScopeContext` describing the scope.
pub fn find_innermost_body<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> (&'a [Stmt], ScopeContext) {
    find_innermost_body_inner(
        body,
        source,
        target_start,
        target_end,
        ScopeKind::Module,
        None,
        None,
    )
}

fn find_innermost_body_inner<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    current_kind: ScopeKind,
    // For Class scope: offset and indent of the parent scope.
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
) -> (&'a [Stmt], ScopeContext) {
    for stmt in body {
        let range = stmt.range();
        let stmt_start = line_of_offset(source, range.start().to_usize());
        let stmt_end = line_of_offset(source, range.end().to_usize().saturating_sub(1));

        if stmt_start <= target_start && stmt_end >= target_end {
            match stmt {
                Stmt::FunctionDef(f) => {
                    return find_innermost_body_inner(
                        &f.body,
                        source,
                        target_start,
                        target_end,
                        ScopeKind::Function,
                        None,
                        None,
                    );
                }
                Stmt::ClassDef(c) => {
                    // Compute the parent indent from the current body.
                    let current_indent = compute_indent(body, source);
                    return find_innermost_body_inner(
                        &c.body,
                        source,
                        target_start,
                        target_end,
                        ScopeKind::Class,
                        Some(range.start().to_usize()),
                        Some(current_indent),
                    );
                }
                _ => {}
            }
        }
    }

    let ctx = make_scope_context(body, source, current_kind, class_def_offset, parent_indent);
    (body, ctx)
}

/// Compute the indentation of the first statement in a body.
fn compute_indent(body: &[Stmt], source: &str) -> String {
    if let Some(first) = body.first() {
        let offset = first.range().start().to_usize();
        let line_start = source[..offset].rfind('\n').map_or(0, |p| p + 1);
        source[line_start..offset].to_string()
    } else {
        String::new()
    }
}

/// Build a `ScopeContext` from a body and its scope metadata.
fn make_scope_context(
    body: &[Stmt],
    source: &str,
    kind: ScopeKind,
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
) -> ScopeContext {
    let (body_start_offset, indent) = if let Some(first) = body.first() {
        let offset = first.range().start().to_usize();
        let line_start = source[..offset].rfind('\n').map_or(0, |p| p + 1);
        (offset, source[line_start..offset].to_string())
    } else {
        (0, String::new())
    };
    ScopeContext {
        kind,
        body_start_offset,
        indent,
        class_def_offset,
        parent_indent,
    }
}

/// Find the parent body (one level above the innermost body containing the target)
/// and its `ScopeContext`. Returns `None` if the target is directly at the top level.
fn find_parent_with_ctx<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> Option<(&'a [Stmt], ScopeContext)> {
    find_parent_with_ctx_inner(
        body,
        source,
        target_start,
        target_end,
        ScopeKind::Module,
        None,
        None,
    )
}

fn find_parent_with_ctx_inner<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    current_kind: ScopeKind,
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
) -> Option<(&'a [Stmt], ScopeContext)> {
    for stmt in body {
        let range = stmt.range();
        let stmt_start = line_of_offset(source, range.start().to_usize());
        let stmt_end = line_of_offset(source, range.end().to_usize().saturating_sub(1));

        if stmt_start <= target_start && stmt_end >= target_end {
            match stmt {
                Stmt::FunctionDef(f) => {
                    // Try to find a deeper parent inside f.body.
                    let deeper = find_parent_with_ctx_inner(
                        &f.body,
                        source,
                        target_start,
                        target_end,
                        ScopeKind::Function,
                        None,
                        None,
                    );
                    if deeper.is_some() {
                        return deeper;
                    }
                    // Target is directly in f.body → current body is the parent.
                    let ctx = make_scope_context(
                        body,
                        source,
                        current_kind,
                        class_def_offset,
                        parent_indent,
                    );
                    return Some((body, ctx));
                }
                Stmt::ClassDef(c) => {
                    let current_indent = compute_indent(body, source);
                    let deeper = find_parent_with_ctx_inner(
                        &c.body,
                        source,
                        target_start,
                        target_end,
                        ScopeKind::Class,
                        Some(range.start().to_usize()),
                        Some(current_indent),
                    );
                    if deeper.is_some() {
                        return deeper;
                    }
                    let ctx = make_scope_context(
                        body,
                        source,
                        current_kind,
                        class_def_offset,
                        parent_indent,
                    );
                    return Some((body, ctx));
                }
                _ => {}
            }
        }
    }
    // Target is directly in this body — no parent exists at a higher level.
    None
}

/// Check if two body slices are the same (by comparing the start offset of their first statement).
fn same_body(a: &[Stmt], b: &[Stmt]) -> bool {
    match (a.first(), b.first()) {
        (Some(x), Some(y)) => x.range().start() == y.range().start(),
        _ => a.is_empty() && b.is_empty(),
    }
}

/// Find the innermost body containing a given byte offset.
///
/// Used by `lib.rs` to find each matched block's body for `after_block` computation.
pub fn find_body_for_block<'a>(
    top_body: &'a [Stmt],
    source: &str,
    block_start_offset: usize,
) -> &'a [Stmt] {
    let line = line_of_offset(source, block_start_offset);
    let (body, _) = find_innermost_body(top_body, source, line, line);
    body
}

/// Determine the appropriate scope context based on where all matched blocks reside.
///
/// If all matches are within the same body, returns the innermost scope context.
/// If matches span sibling scopes, returns the parent scope context.
pub fn find_scope_for_matches(
    top_body: &[Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    matches: &[MatchedBlock],
) -> ScopeContext {
    let (inner_body, inner_ctx) = find_innermost_body(top_body, source, target_start, target_end);

    let all_in_same_body = matches.iter().all(|m| {
        inner_body
            .iter()
            .any(|s| s.range().start().to_usize() == m.start_offset)
    });

    if all_in_same_body {
        return inner_ctx;
    }

    // Matches span sibling scopes — use parent scope context.
    find_parent_with_ctx(top_body, source, target_start, target_end)
        .map(|(_, ctx)| ctx)
        .unwrap_or(inner_ctx)
}

/// A matched block in the source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedBlock {
    /// 1-based start line.
    pub start_line: usize,
    /// 1-based end line (inclusive).
    pub end_line: usize,
    /// Byte offset of the block start.
    pub start_offset: usize,
    /// Byte offset of the block end.
    pub end_offset: usize,
}

/// Scan the file for statement blocks that structurally match
/// the target block (identified by `target_start..=target_end` lines).
///
/// Automatically finds the innermost scope (function/class) containing
/// the target and scans within that scope's body. Also scans sibling
/// scopes (other functions/classes at the same parent level) for matches.
///
/// Returns the list of all matching blocks, **including** the target itself.
pub fn find_matches(
    source: &str,
    target_start: usize,
    target_end: usize,
) -> Result<Vec<MatchedBlock>> {
    if target_start == 0 || target_end == 0 || target_start > target_end {
        bail!("Invalid line range: {target_start}..={target_end}");
    }

    let parsed = parse_module(source).map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;
    let top_body = &parsed.syntax().body;
    let (inner_body, _ctx) = find_innermost_body(top_body, source, target_start, target_end);

    // Compute target hash from the innermost body.
    let target_stmts = select_stmts(source, inner_body, target_start, target_end);
    if target_stmts.is_empty() {
        bail!("No statements found in target range {target_start}..={target_end}");
    }
    let window_size = target_stmts.len();
    let target_hash = hash_stmt_refs(&target_stmts);

    // Scan the innermost body.
    let mut matches = scan_body_with_hash(source, inner_body, target_hash, window_size);

    // Scan sibling scopes at the parent level.
    if let Some((parent_body, _)) = find_parent_with_ctx(top_body, source, target_start, target_end)
    {
        for stmt in parent_body {
            let child_body: Option<&[Stmt]> = match stmt {
                Stmt::FunctionDef(f) => Some(&f.body),
                Stmt::ClassDef(c) => Some(&c.body),
                _ => None,
            };
            if let Some(child) = child_body
                && !same_body(child, inner_body)
            {
                matches.extend(scan_body_with_hash(source, child, target_hash, window_size));
            }
        }
    }

    Ok(matches)
}

/// Scan a body for blocks whose structural hash matches `target_hash`.
fn scan_body_with_hash(
    source: &str,
    body: &[Stmt],
    target_hash: u64,
    window_size: usize,
) -> Vec<MatchedBlock> {
    let mut matches = Vec::new();
    if body.len() < window_size {
        return matches;
    }

    for i in 0..=(body.len() - window_size) {
        let window = &body[i..i + window_size];
        let window_hash = hash_stmts(window);

        if window_hash == target_hash {
            let first = &window[0];
            let last = &window[window_size - 1];
            let start_offset = first.range().start().to_usize();
            let end_offset = last.range().end().to_usize();

            matches.push(MatchedBlock {
                start_line: line_of_offset(source, start_offset),
                end_line: line_of_offset(source, end_offset.saturating_sub(1)),
                start_offset,
                end_offset,
            });
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_target_and_duplicate() {
        let code = "\
a = 1
b = a + 2
c = 3
x = 100
y = x + 200
";
        let matches = find_matches(code, 1, 2).unwrap();
        assert_eq!(matches.len(), 2, "Should find 2 matching blocks");
        assert_eq!(matches[0].start_line, 1);
        assert_eq!(matches[0].end_line, 2);
        assert_eq!(matches[1].start_line, 4);
        assert_eq!(matches[1].end_line, 5);
    }

    #[test]
    fn no_duplicates_returns_only_target() {
        let code = "\
a = 1
b = a + 2
c = a - 3
";
        let matches = find_matches(code, 1, 2).unwrap();
        assert_eq!(matches.len(), 1, "Should find only the target itself");
        assert_eq!(matches[0].start_line, 1);
    }

    #[test]
    fn three_matching_blocks() {
        let code = "\
a = 1
b = a + 2
c = 10
d = c + 20
e = 100
f = e + 200
";
        let matches = find_matches(code, 1, 2).unwrap();
        assert_eq!(matches.len(), 3, "Should find 3 matching blocks");
    }

    #[test]
    fn single_statement_window() {
        let code = "\
x = 1
y = 2
z = 3
";
        // Each line is `VAR = CONST`, all should match.
        let matches = find_matches(code, 1, 1).unwrap();
        assert_eq!(matches.len(), 3, "All 3 single-assignment lines match");
    }

    #[test]
    fn no_false_positives_with_different_operators() {
        let code = "\
a = x + 1
b = y - 2
c = z * 3
";
        let matches = find_matches(code, 1, 1).unwrap();
        assert_eq!(matches.len(), 1, "Different operators should not match");
    }

    #[test]
    fn reports_byte_offsets() {
        let code = "a = 1\nb = 2\n";
        let matches = find_matches(code, 1, 1).unwrap();
        // First statement: bytes 0..5
        assert_eq!(matches[0].start_offset, 0);
        assert_eq!(matches[0].end_offset, 5);
        // Second statement: bytes 6..11
        assert_eq!(matches[1].start_offset, 6);
        assert_eq!(matches[1].end_offset, 11);
    }

    #[test]
    fn scan_inside_function_body() {
        let code = "\
def process():
    a = 1
    b = a + 2
    c = 10
    d = c + 20
";
        let matches = find_matches(code, 2, 3).unwrap();
        assert_eq!(matches.len(), 2, "Should find 2 blocks inside function");
        assert_eq!(matches[0].start_line, 2);
        assert_eq!(matches[1].start_line, 4);
    }

    #[test]
    fn scan_inside_class_method() {
        let code = "\
class Foo:
    def method(self):
        x = 1
        y = x + 2
        a = 10
        b = a + 20
";
        let matches = find_matches(code, 3, 4).unwrap();
        assert_eq!(matches.len(), 2, "Should find 2 blocks inside method");
        assert_eq!(matches[0].start_line, 3);
        assert_eq!(matches[1].start_line, 5);
    }

    #[test]
    fn scan_with_function_calls() {
        let code = "\
foo(x, y)
bar(a, b)
baz(1, 2)
";
        let matches = find_matches(code, 1, 1).unwrap();
        // foo(x,y) and bar(a,b) are both `FUNC(VAR, VAR)` - same structure
        // baz(1,2) is `FUNC(CONST, CONST)` - different
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn scan_across_sibling_functions() {
        let code = "\
def foo():
    a = 1
    b = a + 2

def bar():
    x = 10
    y = x + 20
";
        let matches = find_matches(code, 2, 3).unwrap();
        assert_eq!(
            matches.len(),
            2,
            "Should find matches across sibling functions"
        );
        assert_eq!(matches[0].start_line, 2);
        assert_eq!(matches[1].start_line, 6);
    }

    #[test]
    fn scan_across_sibling_classes() {
        let code = "\
class Foo:
    a = 1
    b = a + 2

class Bar:
    x = 10
    y = x + 20
";
        let matches = find_matches(code, 2, 3).unwrap();
        assert_eq!(
            matches.len(),
            2,
            "Should find matches across sibling classes"
        );
    }

    #[test]
    fn scan_siblings_plus_same_body() {
        let code = "\
def foo():
    a = 1
    b = a + 2
    c = 10
    d = c + 20

def bar():
    x = 100
    y = x + 200
";
        let matches = find_matches(code, 2, 3).unwrap();
        assert_eq!(matches.len(), 3, "2 in foo + 1 in bar");
    }
}
