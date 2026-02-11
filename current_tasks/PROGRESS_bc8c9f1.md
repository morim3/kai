# Progress: Phase 1 - AST Normalization & Hashing

## Current Goal
Phase 1: AST normalization and target block hashing (from design_doc.md)

## Completed
- Project scaffold: `src/main.rs`, `src/lib.rs`, `src/normalize.rs`
- CLI argument parsing (file, start_line, end_line) via clap
- `NormalizeVisitor` implementing `ruff_python_ast::Visitor` with:
  - Variable name normalization (positional IDs: VAR_0, VAR_1, ...)
  - Literal normalization (all literals -> CONSTANT token)
  - Full structural hashing of operators, statement types, expression types
  - ExprContext (Load/Store/Del) hashing for correctness
- `hash_block(source, start_line, end_line)` public API
- Line-range based statement selection
- 10 unit tests, all passing

## Phase 1 Exit Criteria Status
- [x] No crash on parse error
- [x] Equivalent code blocks (differing only in variable names/literals) produce identical hashes
- [x] Different structures produce different hashes

## Failed Approaches
- ruff 0.15.0 requires Rust 1.91+; had to upgrade from 1.89 to 1.93
- `parse_module` returns `Result`, not `Parsed` directly (needed `?` operator)
- `body` is a public field, not a method (`.body` not `.body()`)
- `Ranged` trait must be imported from `ruff_text_size` for `.range()` on AST nodes

## Next Step
- Commit Phase 1 work
- Begin Phase 2: Similar block scanning within a file (sliding window over statements)
