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
    /// The actual innermost body containing the target (may be a control flow body).
    pub inner_body: &'a [Stmt],
    /// The body of the nearest Python scope (Function/Class/Module).
    /// Used for scope-level checks; always a scope body, never a control flow body.
    pub scope_body: &'a [Stmt],
    pub inner_ctx: ScopeContext,
    /// The body one level above the innermost scope (None if innermost == top level).
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
        top_body,
    )
}

#[allow(clippy::too_many_arguments)]
fn find_scopes_inner<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    current_kind: ScopeKind,
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
    scope_body: &'a [Stmt],
) -> ScopeInfo<'a> {
    for stmt in body {
        let range = stmt.range();
        let stmt_start = line_of_offset(source, range.start().to_usize());
        let stmt_end = line_of_offset(source, range.end().to_usize().saturating_sub(1));

        if stmt_start <= target_start && stmt_end >= target_end {
            // Check for scope-creating children (FunctionDef, ClassDef).
            let child = match stmt {
                Stmt::FunctionDef(f) => Some((f.body.as_slice(), ScopeKind::Function, None, None)),
                Stmt::ClassDef(c) => Some((
                    c.body.as_slice(),
                    ScopeKind::Class,
                    Some(range.start().to_usize()),
                    scope_body
                        .first()
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
                    child_body,
                );
                // If no parent was found deeper, the current scope is the parent.
                if info.parent.is_none() {
                    let ctx = make_scope_context(
                        scope_body,
                        source,
                        current_kind,
                        class_def_offset,
                        parent_indent,
                    );
                    info.parent = Some((scope_body, ctx));
                }
                return info;
            }

            // Check for control flow children. These don't create Python scopes,
            // so we recurse with the same scope parameters.
            for sub_body in control_flow_bodies(stmt) {
                if body_contains_lines(sub_body, source, target_start, target_end) {
                    return find_scopes_inner(
                        sub_body,
                        source,
                        target_start,
                        target_end,
                        current_kind,
                        class_def_offset,
                        parent_indent.clone(),
                        scope_body,
                    );
                }
            }
        }
    }

    // Base case: target is directly in this body.
    let ctx = make_scope_context(
        scope_body,
        source,
        current_kind,
        class_def_offset,
        parent_indent,
    );
    ScopeInfo {
        inner_body: body,
        scope_body,
        inner_ctx: ctx,
        parent: None,
    }
}

/// Collect sub-bodies from control flow statements (not scope-creating statements).
fn control_flow_bodies(stmt: &Stmt) -> Vec<&[Stmt]> {
    match stmt {
        Stmt::If(s) => {
            let mut bodies: Vec<&[Stmt]> = vec![&s.body];
            for clause in &s.elif_else_clauses {
                bodies.push(&clause.body);
            }
            bodies
        }
        Stmt::For(s) => vec![&s.body, &s.orelse],
        Stmt::While(s) => vec![&s.body, &s.orelse],
        Stmt::With(s) => vec![&s.body],
        Stmt::Try(s) => {
            let mut bodies: Vec<&[Stmt]> = vec![&s.body];
            for handler in &s.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                bodies.push(&h.body);
            }
            bodies.push(&s.orelse);
            bodies.push(&s.finalbody);
            bodies
        }
        Stmt::Match(s) => s.cases.iter().map(|c| c.body.as_slice()).collect(),
        _ => vec![],
    }
}

/// Check if a body's line range contains the target lines.
fn body_contains_lines(
    body: &[Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> bool {
    let (Some(first), Some(last)) = (body.first(), body.last()) else {
        return false;
    };
    let body_start = line_of_offset(source, first.range().start().to_usize());
    let body_end = line_of_offset(source, last.range().end().to_usize().saturating_sub(1));
    body_start <= target_start && body_end >= target_end
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
/// If all matches are within the same scope, returns the innermost scope context.
/// If matches span sibling scopes, returns the parent scope context.
pub fn find_scope_for_matches(
    top_body: &[Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    matches: &[MatchedBlock],
) -> ScopeContext {
    let info = find_scopes(top_body, source, target_start, target_end);

    // Check if all matches fall within the byte range of the target's scope body.
    // This correctly handles matches in different control flow branches within the
    // same scope (e.g., if/else bodies inside the same function).
    let all_in_same_scope =
        if let (Some(first), Some(last)) = (info.scope_body.first(), info.scope_body.last()) {
            let scope_start = first.range().start().to_usize();
            let scope_end = last.range().end().to_usize();
            matches
                .iter()
                .all(|m| m.start_offset >= scope_start && m.start_offset < scope_end)
        } else {
            true
        };

    if all_in_same_scope {
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

/// Scan the target file for blocks matching the target range, returning the hash and window size.
///
/// This is the first stage of the multi-file pipeline: it computes the structural hash
/// of the target block, scans the target file, and returns all the information needed
/// to scan additional files.
pub fn find_matches_with_hash(
    source: &str,
    target_start: usize,
    target_end: usize,
) -> Result<(u64, usize, Vec<MatchedBlock>)> {
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
    let target_hash = hash_stmt_refs(&target_stmts, source);

    // Scan the widest relevant scope: parent body (if it exists) or scope body.
    // This recursively covers the innermost body, sibling scopes, and parent-level statements.
    let search_root = match &scope_info.parent {
        Some((parent_body, _)) => parent_body,
        None => scope_info.scope_body,
    };
    let mut matches = Vec::new();
    scan_all_bodies_recursive(source, search_root, target_hash, window_size, &mut matches);

    Ok((target_hash, window_size, matches))
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
    let (_, _, matches) = find_matches_with_hash(source, target_start, target_end)?;
    Ok(matches)
}

/// Scan an arbitrary source file for blocks matching a known structural hash.
///
/// Recursively traverses all scopes (module, functions, classes) in the source.
/// Used to find matches in files other than the target file.
pub fn find_matches_in_file(
    source: &str,
    target_hash: u64,
    window_size: usize,
) -> Vec<MatchedBlock> {
    let Ok(parsed) = crate::parse_python(source) else {
        return Vec::new();
    };
    let top_body = &parsed.syntax().body;
    let mut matches = Vec::new();
    scan_all_bodies_recursive(source, top_body, target_hash, window_size, &mut matches);
    matches
}

/// Recursively scan all bodies (module, function, class, control flow) in the AST
/// for matching blocks.
fn scan_all_bodies_recursive(
    source: &str,
    body: &[Stmt],
    target_hash: u64,
    window_size: usize,
    matches: &mut Vec<MatchedBlock>,
) {
    // Scan this body.
    matches.extend(scan_body_with_hash(source, body, target_hash, window_size));

    // Recurse into child scopes and control flow bodies.
    for stmt in body {
        let recurse = |b: &[Stmt], m: &mut Vec<MatchedBlock>| {
            scan_all_bodies_recursive(source, b, target_hash, window_size, m);
        };
        match stmt {
            Stmt::FunctionDef(f) => recurse(&f.body, matches),
            Stmt::ClassDef(c) => recurse(&c.body, matches),
            Stmt::If(if_stmt) => {
                recurse(&if_stmt.body, matches);
                for clause in &if_stmt.elif_else_clauses {
                    recurse(&clause.body, matches);
                }
            }
            Stmt::For(for_stmt) => {
                recurse(&for_stmt.body, matches);
                recurse(&for_stmt.orelse, matches);
            }
            Stmt::While(while_stmt) => {
                recurse(&while_stmt.body, matches);
                recurse(&while_stmt.orelse, matches);
            }
            Stmt::With(with_stmt) => recurse(&with_stmt.body, matches),
            Stmt::Try(try_stmt) => {
                recurse(&try_stmt.body, matches);
                for handler in &try_stmt.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
                    recurse(&h.body, matches);
                }
                recurse(&try_stmt.orelse, matches);
                recurse(&try_stmt.finalbody, matches);
            }
            Stmt::Match(match_stmt) => {
                for case in &match_stmt.cases {
                    recurse(&case.body, matches);
                }
            }
            _ => {}
        }
    }
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

    let mut i = 0;
    while i <= body.len() - window_size {
        let window = &body[i..i + window_size];
        let window_hash = hash_stmts(window, source);

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
            // Skip past this match to prevent overlapping matches.
            i += window_size;
        } else {
            i += 1;
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert find_matches returns expected count and optional line positions.
    fn assert_matches(code: &str, start: usize, end: usize, expected: &[(usize, usize)]) {
        let matches = find_matches(code, start, end).unwrap();
        assert_eq!(
            matches.len(),
            expected.len(),
            "Expected {} matches, got {}",
            expected.len(),
            matches.len()
        );
        for (m, &(exp_start, exp_end)) in matches.iter().zip(expected) {
            assert_eq!(m.start_line, exp_start);
            assert_eq!(m.end_line, exp_end);
        }
    }

    #[test]
    fn module_level_matching() {
        // 2 matching blocks
        assert_matches(
            "a = 1\nb = a + 2\nc = 3\nx = 100\ny = x + 200\n",
            1,
            2,
            &[(1, 2), (4, 5)],
        );
        // 3 matching blocks
        assert_matches(
            "a = 1\nb = a + 2\nc = 10\nd = c + 20\ne = 100\nf = e + 200\n",
            1,
            2,
            &[(1, 2), (3, 4), (5, 6)],
        );
        // No duplicates — only target returned
        assert_matches("a = 1\nb = a + 2\nc = a - 3\n", 1, 2, &[(1, 2)]);
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
    fn scan_across_siblings() {
        // Sibling functions
        assert_matches(
            "def foo():\n    a = 1\n    b = a + 2\n\ndef bar():\n    x = 10\n    y = x + 20\n",
            2,
            3,
            &[(2, 3), (6, 7)],
        );
        // Sibling classes
        assert_matches(
            "class Foo:\n    a = 1\n    b = a + 2\n\nclass Bar:\n    x = 10\n    y = x + 20\n",
            2,
            3,
            &[(2, 3), (6, 7)],
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

    #[test]
    fn no_overlapping_matches() {
        // 3 identical statements, window_size=2: should get (1,2) only, not (1,2)+(2,3).
        let code = "a = 1\nb = 2\nc = 3\n";
        assert_matches(code, 1, 2, &[(1, 2)]);
    }

    #[test]
    fn scan_child_finds_parent_body_matches() {
        // Reference inside function, matching block at module level (parent body).
        let code = "\
a = 1
b = a + 2

def foo():
    x = 10
    y = x + 20
";
        assert_matches(code, 5, 6, &[(1, 2), (5, 6)]);
    }

    #[test]
    fn find_matches_with_hash_returns_hash_and_window() {
        let code = "\
a = 1
b = a + 2
c = 10
d = c + 20
";
        let (hash, window_size, matches) = find_matches_with_hash(code, 1, 2).unwrap();
        assert!(hash != 0, "Hash should be non-zero");
        assert_eq!(window_size, 2);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn find_matches_in_file_module_level() {
        let target = "\
a = 1
b = a + 2
";
        let (hash, window, _) = find_matches_with_hash(target, 1, 2).unwrap();

        let other = "\
x = 10
y = x + 20
z = 100
";
        let matches = find_matches_in_file(other, hash, window);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start_line, 1);
        assert_eq!(matches[0].end_line, 2);
    }

    #[test]
    fn find_matches_in_file_nested_scope() {
        let target = "\
a = 1
b = a + 2
";
        let (hash, window, _) = find_matches_with_hash(target, 1, 2).unwrap();

        let other = "\
def foo():
    x = 10
    y = x + 20

class Bar:
    p = 100
    q = p + 200
";
        let matches = find_matches_in_file(other, hash, window);
        assert_eq!(
            matches.len(),
            2,
            "Should find matches in function and class"
        );
    }

    #[test]
    fn scan_module_into_child_scopes() {
        // Reference block at module level should find matches inside functions and class methods.
        let code = "\
a = 1
b = a + 2
x = 10
y = x + 20

def foo():
    p = 100
    q = p + 200

class Bar:
    def method(self):
        m = 1000
        n = m + 2000
";
        assert_matches(code, 1, 2, &[(1, 2), (3, 4), (7, 8), (12, 13)]);
    }

    #[test]
    fn scan_sibling_into_nested_child() {
        // Reference inside foo(); sibling class has a matching block in a nested method.
        let code = "\
def foo():
    a = 1
    b = a + 2

class Bar:
    def method(self):
        x = 10
        y = x + 20
";
        assert_matches(code, 2, 3, &[(2, 3), (7, 8)]);
    }

    #[test]
    fn find_matches_in_file_no_match() {
        let target = "\
a = 1
b = a + 2
";
        let (hash, window, _) = find_matches_with_hash(target, 1, 2).unwrap();

        let other = "\
x = 1 - 2
y = 3 * 4
";
        let matches = find_matches_in_file(other, hash, window);
        assert!(matches.is_empty());
    }
}
