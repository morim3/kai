use anyhow::{Result, bail};
use ruff_python_ast::Stmt;
use ruff_text_size::Ranged;

use crate::normalize::{
    hash_stmt_refs, hash_stmts, indent_at_offset, line_of_offset, select_stmts,
};

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

/// Result of a scope traversal: the innermost body/context and optionally the parent.
pub struct ScopeInfo<'a> {
    pub inner_body: &'a [Stmt],
    pub inner_ctx: ScopeContext,
    /// The body one level above the innermost (None if innermost == top level).
    pub parent: Option<(&'a [Stmt], ScopeContext)>,
}

/// Traverse the AST to find both the innermost scope body containing the target
/// and the parent scope (one level up). A single traversal produces both results.
pub fn find_scopes<'a>(
    top_body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> ScopeInfo<'a> {
    find_scopes_inner(
        top_body,
        source,
        target_start,
        target_end,
        ScopeKind::Module,
        None,
        None,
    )
}

fn find_scopes_inner<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    current_kind: ScopeKind,
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
) -> ScopeInfo<'a> {
    for stmt in body {
        let range = stmt.range();
        let stmt_start = line_of_offset(source, range.start().to_usize());
        let stmt_end = line_of_offset(source, range.end().to_usize().saturating_sub(1));

        if stmt_start <= target_start && stmt_end >= target_end {
            let child = match stmt {
                Stmt::FunctionDef(f) => Some((f.body.as_slice(), ScopeKind::Function, None, None)),
                Stmt::ClassDef(c) => Some((
                    c.body.as_slice(),
                    ScopeKind::Class,
                    Some(range.start().to_usize()),
                    body.first()
                        .map(|s| indent_at_offset(source, s.range().start().to_usize())),
                )),
                _ => None,
            };

            if let Some((child_body, child_kind, child_class_offset, child_parent_indent)) = child {
                let mut info = find_scopes_inner(
                    child_body,
                    source,
                    target_start,
                    target_end,
                    child_kind,
                    child_class_offset,
                    child_parent_indent,
                );
                // If no parent was found deeper, the current body is the parent.
                if info.parent.is_none() {
                    let ctx = make_scope_context(
                        body,
                        source,
                        current_kind,
                        class_def_offset,
                        parent_indent,
                    );
                    info.parent = Some((body, ctx));
                }
                return info;
            }
        }
    }

    // Base case: target is directly in this body.
    let ctx = make_scope_context(body, source, current_kind, class_def_offset, parent_indent);
    ScopeInfo {
        inner_body: body,
        inner_ctx: ctx,
        parent: None,
    }
}

/// Convenience wrapper: returns only the innermost body and its scope context.
pub fn find_innermost_body<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> (&'a [Stmt], ScopeContext) {
    let info = find_scopes(body, source, target_start, target_end);
    (info.inner_body, info.inner_ctx)
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
        (offset, indent_at_offset(source, offset))
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
    let info = find_scopes(top_body, source, target_start, target_end);

    let all_in_same_body = matches.iter().all(|m| {
        info.inner_body
            .iter()
            .any(|s| s.range().start().to_usize() == m.start_offset)
    });

    if all_in_same_body {
        return info.inner_ctx;
    }

    // Matches span sibling scopes — use parent scope context.
    info.parent.map(|(_, ctx)| ctx).unwrap_or(info.inner_ctx)
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

    let parsed = crate::parse_python(source)?;
    let top_body = &parsed.syntax().body;
    let scope_info = find_scopes(top_body, source, target_start, target_end);

    // Compute target hash from the innermost body.
    let target_stmts = select_stmts(source, scope_info.inner_body, target_start, target_end);
    if target_stmts.is_empty() {
        bail!("No statements found in target range {target_start}..={target_end}");
    }
    let window_size = target_stmts.len();
    let target_hash = hash_stmt_refs(&target_stmts);

    // Scan the innermost body.
    let mut matches = scan_body_with_hash(source, scope_info.inner_body, target_hash, window_size);

    // Scan sibling scopes at the parent level.
    if let Some((parent_body, _)) = scope_info.parent {
        for stmt in parent_body {
            let child_body: Option<&[Stmt]> = match stmt {
                Stmt::FunctionDef(f) => Some(&f.body),
                Stmt::ClassDef(c) => Some(&c.body),
                _ => None,
            };
            if let Some(child) = child_body
                && !same_body(child, scope_info.inner_body)
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
