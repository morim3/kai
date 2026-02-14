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

/// Result of a scope traversal: the innermost body and its scope context.
pub struct ScopeInfo<'a> {
    /// The actual innermost body containing the target (may be a control flow body).
    pub inner_body: &'a [Stmt],
    /// The body of the nearest Python scope (Function/Class/Module).
    /// Used for scope-level checks; always a scope body, never a control flow body.
    pub scope_body: &'a [Stmt],
    pub inner_ctx: ScopeContext,
}

/// Traverse the AST to find the innermost scope body containing the target.
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
            if let Some((child_body, child_kind, child_class_offset, child_parent_indent)) =
                child_scope_info(stmt, source, scope_body)
            {
                return find_scopes_inner(
                    child_body,
                    source,
                    target_start,
                    target_end,
                    child_kind,
                    child_class_offset,
                    child_parent_indent,
                    child_body,
                );
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
    }
}

type ChildScopeInfo<'a> = (&'a [Stmt], ScopeKind, Option<usize>, Option<String>);

fn child_scope_info<'a>(
    stmt: &'a Stmt,
    source: &str,
    enclosing_body: &[Stmt],
) -> Option<ChildScopeInfo<'a>> {
    match stmt {
        Stmt::FunctionDef(f) => Some((f.body.as_slice(), ScopeKind::Function, None, None)),
        Stmt::ClassDef(c) => {
            let offset = stmt.range().start().to_usize();
            let parent_indent = enclosing_body
                .first()
                .map(|s| indent_at_offset(source, s.range().start().to_usize()));
            Some((
                c.body.as_slice(),
                ScopeKind::Class,
                Some(offset),
                parent_indent,
            ))
        }
        _ => None,
    }
}

/// Extract all child statement bodies from a statement (both scope-creating and control flow).
fn all_statement_bodies(stmt: &Stmt) -> Vec<&[Stmt]> {
    match stmt {
        Stmt::FunctionDef(f) => vec![f.body.as_slice()],
        Stmt::ClassDef(c) => vec![c.body.as_slice()],
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

/// Collect sub-bodies from control flow statements (not scope-creating statements).
fn control_flow_bodies(stmt: &Stmt) -> Vec<&[Stmt]> {
    match stmt {
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => vec![],
        _ => all_statement_bodies(stmt),
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

/// Collect all statements that come after a block, up to the Python scope boundary.
///
/// Python control flow (for/if/while/with/try) does not create variable scopes.
/// When a block is nested inside control flow, variables stored in the block may
/// be used outside that control flow structure. This function recursively descends
/// through control flow bodies to find the block, then collects all statements
/// that execute after it at each nesting level.
///
/// Example: if a block is inside a `for` loop body, this returns:
/// - Statements after the block within the `for` body
/// - Statements after the `for` loop in the enclosing scope
/// - (and so on, up to the scope boundary)
pub fn collect_after_stmts(
    scope_body: &[Stmt],
    block_start_offset: usize,
    window_size: usize,
) -> Vec<&Stmt> {
    let mut result = Vec::new();
    collect_after_inner(scope_body, block_start_offset, window_size, &mut result);
    result
}

/// Recursive helper for `collect_after_stmts`.
/// Returns `true` if the block was found in this body.
fn collect_after_inner<'a>(
    body: &'a [Stmt],
    block_start_offset: usize,
    window_size: usize,
    result: &mut Vec<&'a Stmt>,
) -> bool {
    for (i, stmt) in body.iter().enumerate() {
        let stmt_start = stmt.range().start().to_usize();
        let stmt_end = stmt.range().end().to_usize();

        // Direct match: block starts at this statement.
        if stmt_start == block_start_offset {
            // Add all statements after the block window.
            let after_start = i + window_size;
            for after_stmt in body.iter().skip(after_start) {
                result.push(after_stmt);
            }
            return true;
        }

        // Block is nested inside this statement (control flow).
        if stmt_start < block_start_offset && block_start_offset < stmt_end {
            for sub_body in control_flow_bodies(stmt) {
                if collect_after_inner(sub_body, block_start_offset, window_size, result) {
                    // Block found inside this control flow body.
                    // Add all statements after this control flow statement.
                    for after_stmt in body.iter().skip(i + 1) {
                        result.push(after_stmt);
                    }
                    return true;
                }
            }
        }
    }
    false
}

/// Determine the appropriate scope context for the extracted function.
///
/// Finds the narrowest Python scope (function/class/module) that contains ALL
/// matched blocks. This is the lowest common ancestor (LCA) of all matches
/// in the scope tree. Works correctly for any nesting depth.
pub fn find_scope_for_matches(
    top_body: &[Stmt],
    source: &str,
    matches: &[MatchedBlock],
) -> ScopeContext {
    find_common_scope(top_body, source, matches, ScopeKind::Module, None, None)
}

/// Recursively descend through scope-creating nodes (FunctionDef/ClassDef)
/// as long as a single child scope contains ALL matches. When no single child
/// scope contains all matches, the current level is the LCA.
fn find_common_scope(
    body: &[Stmt],
    source: &str,
    matches: &[MatchedBlock],
    kind: ScopeKind,
    class_def_offset: Option<usize>,
    parent_indent: Option<String>,
) -> ScopeContext {
    for stmt in body {
        // Only consider scope-creating statements (FunctionDef, ClassDef).
        // Control flow (for/if/while/with/try) does not create Python scopes.
        if let Some((child_body, child_kind, child_class_offset, child_parent_indent)) =
            child_scope_info(stmt, source, body)
        {
            let range = stmt.range();
            let stmt_start = range.start().to_usize();
            let stmt_end = range.end().to_usize();
            if matches
                .iter()
                .all(|m| m.start_offset >= stmt_start && m.start_offset < stmt_end)
            {
                // All matches are within this child scope — descend deeper.
                return find_common_scope(
                    child_body,
                    source,
                    matches,
                    child_kind,
                    child_class_offset,
                    child_parent_indent,
                );
            }
        }
    }

    make_scope_context(body, source, kind, class_def_offset, parent_indent)
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

    // Scan the entire file from the module body. scan_all_bodies_recursive
    // descends into all scopes (functions, classes) and control flow bodies,
    // so this finds matches at any nesting depth.
    let mut matches = Vec::new();
    scan_all_bodies_recursive(source, top_body, target_hash, window_size, &mut matches);

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

/// A matched block tagged with the index of the source file it came from.
#[derive(Debug, Clone)]
pub struct SourcedBlock {
    pub block: MatchedBlock,
    /// 0 = target file, 1+ = additional files.
    pub source_index: usize,
}

/// Scan target file and optional extra files for matching blocks.
///
/// Returns all matching blocks tagged with their source file index.
/// `sources[0]` is the target file; remaining entries are extra files to search.
pub fn scan_all_sources(
    sources: &[&str],
    start_line: usize,
    end_line: usize,
) -> Result<Vec<SourcedBlock>> {
    let (target_hash, window_size, target_matches) =
        find_matches_with_hash(sources[0], start_line, end_line)?;

    let mut all_blocks: Vec<SourcedBlock> = target_matches
        .into_iter()
        .map(|b| SourcedBlock {
            block: b,
            source_index: 0,
        })
        .collect();

    for (i, src) in sources.iter().enumerate().skip(1) {
        let extra = find_matches_in_file(src, target_hash, window_size);
        all_blocks.extend(extra.into_iter().map(|b| SourcedBlock {
            block: b,
            source_index: i,
        }));
    }

    if all_blocks.len() < 2 {
        bail!(
            "Only {} block(s) found. Need at least 2 matching blocks to extract a function.",
            all_blocks.len()
        );
    }

    Ok(all_blocks)
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
    matches.extend(scan_body_with_hash(source, body, target_hash, window_size));

    for stmt in body {
        let recurse = |b: &[Stmt], m: &mut Vec<MatchedBlock>| {
            scan_all_bodies_recursive(source, b, target_hash, window_size, m);
        };

        for child_body in all_statement_bodies(stmt) {
            recurse(child_body, matches);
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
    fn scope_for_matches_same_function() {
        let code = "\
def foo():
    a = 1
    b = a + 2
    x = 10
    y = x + 20
";
        let parsed = crate::parse_python(code).unwrap();
        let body = &parsed.syntax().body;
        let matches = find_matches(code, 2, 3).unwrap();
        let ctx = find_scope_for_matches(body, code, &matches);
        assert_eq!(ctx.kind, ScopeKind::Function);
    }

    #[test]
    fn scope_for_matches_across_classes() {
        // Matches in different classes → LCA is module scope.
        let code = "\
class A:
    def foo(self):
        x = 1
        print(x)

class B:
    def bar(self):
        y = 10
        print(y)
";
        let parsed = crate::parse_python(code).unwrap();
        let body = &parsed.syntax().body;
        let matches = find_matches(code, 3, 4).unwrap();
        assert_eq!(matches.len(), 2);
        let ctx = find_scope_for_matches(body, code, &matches);
        assert_eq!(ctx.kind, ScopeKind::Module);
    }

    #[test]
    fn scope_for_matches_across_sibling_functions() {
        let code = "\
def foo():
    a = 1
    b = a + 2

def bar():
    x = 10
    y = x + 20
";
        let parsed = crate::parse_python(code).unwrap();
        let body = &parsed.syntax().body;
        let matches = find_matches(code, 2, 3).unwrap();
        assert_eq!(matches.len(), 2);
        let ctx = find_scope_for_matches(body, code, &matches);
        assert_eq!(ctx.kind, ScopeKind::Module);
    }

    #[test]
    fn scan_across_three_level_nesting() {
        // Block inside Class→Method (3 levels: Module→Class→Function).
        // Matches should be found across separate top-level classes.
        let code = "\
class Animal:
    @classmethod
    def create(cls):
        name = \"dog\"
        obj = cls(name)

class Vehicle:
    @classmethod
    def make(cls):
        label = \"car\"
        obj = cls(label)
";
        assert_matches(code, 4, 5, &[(4, 5), (10, 11)]);
    }

    #[test]
    fn scan_across_nested_functions() {
        // Block inside Function→Function (3 levels: Module→Function→Function).
        let code = "\
def outer_a():
    def inner():
        x = 1
        print(x)

def outer_b():
    def helper():
        y = 10
        print(y)
";
        assert_matches(code, 3, 4, &[(3, 4), (8, 9)]);
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
