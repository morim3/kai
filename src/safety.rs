use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::Ranged;

/// A flow-control statement that is unsafe to extract into a function.
#[derive(Debug, Clone)]
pub struct UnsafeNode {
    pub kind: UnsafeKind,
    /// Byte offset in the source (for error messages).
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeKind {
    Break,
    Continue,
    Return,
    Yield,
    YieldFrom,
}

impl UnsafeKind {
    fn label(self) -> &'static str {
        match self {
            UnsafeKind::Break => "break",
            UnsafeKind::Continue => "continue",
            UnsafeKind::Return => "return",
            UnsafeKind::Yield => "yield",
            UnsafeKind::YieldFrom => "yield from",
        }
    }
}

/// Check whether a block of statements can be safely extracted into a function.
///
/// Returns `Ok(())` if safe, or `Err(nodes)` listing the unsafe flow-control statements.
pub fn check_extractable(stmts: &[Stmt]) -> Result<(), Vec<UnsafeNode>> {
    let mut checker = SafetyChecker {
        loop_depth: 0,
        function_depth: 0,
        unsafe_nodes: Vec::new(),
    };
    for stmt in stmts {
        checker.visit_stmt(stmt);
    }
    if checker.unsafe_nodes.is_empty() {
        Ok(())
    } else {
        Err(checker.unsafe_nodes)
    }
}

/// Format unsafe nodes into a human-readable error message.
pub fn format_unsafe_error(source: &str, nodes: &[UnsafeNode]) -> String {
    let descriptions: Vec<String> = nodes
        .iter()
        .map(|n| {
            let line = crate::normalize::line_of_offset(source, n.offset);
            format!("'{}' at line {line}", n.kind.label())
        })
        .collect();
    format!(
        "Cannot extract: block contains {} (not inside a matching scope within the block)",
        descriptions.join(", ")
    )
}

struct SafetyChecker {
    loop_depth: usize,
    function_depth: usize,
    unsafe_nodes: Vec<UnsafeNode>,
}

impl<'a> Visitor<'a> for SafetyChecker {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Break(b) => {
                if self.loop_depth == 0 && self.function_depth == 0 {
                    self.unsafe_nodes.push(UnsafeNode {
                        kind: UnsafeKind::Break,
                        offset: b.range().start().to_usize(),
                    });
                }
            }
            Stmt::Continue(c) => {
                if self.loop_depth == 0 && self.function_depth == 0 {
                    self.unsafe_nodes.push(UnsafeNode {
                        kind: UnsafeKind::Continue,
                        offset: c.range().start().to_usize(),
                    });
                }
            }
            Stmt::Return(r) => {
                if self.function_depth == 0 {
                    self.unsafe_nodes.push(UnsafeNode {
                        kind: UnsafeKind::Return,
                        offset: r.range().start().to_usize(),
                    });
                }
            }
            Stmt::For(_) | Stmt::While(_) => {
                self.loop_depth += 1;
                walk_stmt(self, stmt);
                self.loop_depth -= 1;
            }
            Stmt::FunctionDef(_) => {
                self.function_depth += 1;
                walk_stmt(self, stmt);
                self.function_depth -= 1;
            }
            _ => {
                walk_stmt(self, stmt);
            }
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Yield(y) => {
                if self.function_depth == 0 {
                    self.unsafe_nodes.push(UnsafeNode {
                        kind: UnsafeKind::Yield,
                        offset: y.range().start().to_usize(),
                    });
                }
            }
            Expr::YieldFrom(y) => {
                if self.function_depth == 0 {
                    self.unsafe_nodes.push(UnsafeNode {
                        kind: UnsafeKind::YieldFrom,
                        offset: y.range().start().to_usize(),
                    });
                }
            }
            Expr::Lambda(_) => {
                self.function_depth += 1;
                walk_expr(self, expr);
                self.function_depth -= 1;
            }
            _ => {
                walk_expr(self, expr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_stmts;

    #[test]
    fn unsafe_flow_detected() {
        let cases: &[(&str, UnsafeKind)] = &[
            ("break", UnsafeKind::Break),
            ("continue", UnsafeKind::Continue),
            ("return x", UnsafeKind::Return),
            ("yield x", UnsafeKind::Yield),
            ("yield from gen()", UnsafeKind::YieldFrom),
        ];
        for (code, expected_kind) in cases {
            // Wrap break/continue in a valid loop for parsing
            let (parse_code, check_inner) = match *expected_kind {
                UnsafeKind::Break | UnsafeKind::Continue => {
                    (format!("for _ in x:\n    {code}"), true)
                }
                _ => (code.to_string(), false),
            };
            let stmts = parse_stmts(&parse_code);
            // For break/continue, check the inner body of the for loop
            let check_stmts = if check_inner {
                if let Stmt::For(f) = &stmts[0] {
                    f.body.as_slice()
                } else {
                    unreachable!()
                }
            } else {
                stmts.as_slice()
            };
            let err = check_extractable(check_stmts).unwrap_err();
            assert_eq!(
                err[0].kind, *expected_kind,
                "{code}: expected {expected_kind:?}"
            );
        }
    }

    #[test]
    fn safe_when_nested() {
        let cases: &[(&str, &str)] = &[
            ("for x in y:\n    break", "break inside block's own loop"),
            (
                "while True:\n    continue",
                "continue inside block's own loop",
            ),
            ("def f():\n    return x", "return inside nested function"),
            ("f = lambda: (yield x)", "yield inside lambda"),
            ("x = 1\ny = x + 2", "no flow control at all"),
        ];
        for (code, label) in cases {
            let stmts = parse_stmts(code);
            assert!(check_extractable(&stmts).is_ok(), "{label}: should be safe");
        }
    }
}
