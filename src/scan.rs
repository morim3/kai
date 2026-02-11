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
}

/// Find the innermost scope body containing the given line range.
///
/// Recursively drills into `FunctionDef`, `AsyncFunctionDef`, and `ClassDef`
/// to find the deepest body that fully contains `target_start..=target_end`.
///
/// Returns the body slice and a `ScopeContext` describing the scope.
pub fn find_innermost_body<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
) -> (&'a [Stmt], ScopeContext) {
    find_innermost_body_inner(body, source, target_start, target_end, ScopeKind::Module)
}

fn find_innermost_body_inner<'a>(
    body: &'a [Stmt],
    source: &str,
    target_start: usize,
    target_end: usize,
    current_kind: ScopeKind,
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
                    );
                }
                Stmt::ClassDef(c) => {
                    return find_innermost_body_inner(
                        &c.body,
                        source,
                        target_start,
                        target_end,
                        ScopeKind::Class,
                    );
                }
                _ => {}
            }
        }
    }

    let (body_start_offset, indent) = if let Some(first) = body.first() {
        let offset = first.range().start().to_usize();
        let line_start = source[..offset].rfind('\n').map_or(0, |p| p + 1);
        (offset, source[line_start..offset].to_string())
    } else {
        (0, String::new())
    };

    let ctx = ScopeContext {
        kind: current_kind,
        body_start_offset,
        indent,
    };
    (body, ctx)
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
/// the target and scans within that scope's body.
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
    let (body, _ctx) = find_innermost_body(top_body, source, target_start, target_end);

    scan_body(source, body, target_start, target_end)
}

/// Scan a body (slice of statements) for blocks matching the target line range.
fn scan_body(
    source: &str,
    body: &[Stmt],
    target_start: usize,
    target_end: usize,
) -> Result<Vec<MatchedBlock>> {
    let target_stmts = select_stmts(source, body, target_start, target_end);
    if target_stmts.is_empty() {
        bail!("No statements found in target range {target_start}..={target_end}");
    }

    let window_size = target_stmts.len();
    let target_hash = hash_stmt_refs(&target_stmts);

    let mut matches = Vec::new();
    if body.len() < window_size {
        return Ok(matches);
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

    Ok(matches)
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
}
