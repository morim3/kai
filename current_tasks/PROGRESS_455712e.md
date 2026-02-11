# Progress: All Phases Complete

## Current Goal
All 4 phases from design_doc.md are implemented and tested.

## Completed

### Phase 1: AST Normalization & Hashing
- `src/normalize.rs`: NormalizeVisitor, hash_block(), hash_stmts()
- 10 tests passing

### Phase 2: Similar Block Scanning
- `src/scan.rs`: find_matches() with sliding window, MatchedBlock
- 7 tests passing

### Phase 3: Variable Scope Analysis
- `src/scope.rs`: analyze_block(), unify_signatures(), VarCollector
- 9 tests passing

### Phase 4: Code Rewriting & Diff Output
- `src/diff_extract.rs`: extract_divergences() for literal/name differences
- `src/rewrite.rs`: generate_function_def(), generate_call(), apply_refactoring(), unified_diff()
- `src/main.rs`: Full CLI pipeline with --write flag
- 11 tests passing (4 diff_extract + 7 rewrite)

## Total: 37 tests, all passing

## Exit Criteria Status
- [x] Phase 1: Equivalent code hashes match; different structures don't
- [x] Phase 2: All matching blocks found with line/byte positions
- [x] Phase 3: Correct inputs/outputs; unified function signature
- [x] Phase 4: Valid Python output; unified diff; --write mode works

## Failed Approaches & Learnings
- ruff 0.15.0 requires Rust 1.91+ (upgraded from 1.89 to 1.93)
- `parse_module` returns Result (not Parsed directly)
- `body` is a field, not a method
- `Ranged` trait needs import from `ruff_text_size`
- `ExprCall.arguments.args` (not `ExprCall.args`)
- `ExprList`/`ExprTuple` can't be OR-matched in match arms (different types)
- Literal values in `Divergence` must use source text (via TextRange), not Debug format
- Must visit RHS before LHS in assignments for correct input detection

## Next Steps
- Consider edge cases: nested functions, class methods, multi-file support
- Performance optimization for large files
- Better function naming heuristics
