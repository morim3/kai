# PROGRESS — Iter 5: Multi-File Extract Method

## Current Goal
Iter 5: 複数ファイル対応（Multi-File Extract Method）

## Status: ✅ COMPLETE

## What Was Completed
1. **scan.rs**: `find_matches_with_hash()`, `find_matches_in_file()`, `scan_all_bodies_recursive()` 追加。既存 `find_matches()` をラッパー化。
2. **lib.rs**: `SourcedBlock` 構造体、`plan_extraction_multi()` 追加。クロスファイル時は Module スコープに強制。既存 `plan_extraction()` をラッパー化。
3. **rewrite.rs**: `generate_import()`, `find_import_insert_point()`, `apply_refactoring_multi()`, `replace_blocks_with_calls()`, `remap_signature()` 追加。
4. **main.rs**: CLI を `pym FILE [FILE...] START END` 形式に変更。単一ファイル完全互換。
5. **tests**: 5つのマルチファイルフィクスチャ追加。integration.rs をマルチファイル対応に拡張。
6. **design_doc.md**: Iter 5 を ✅ に更新。

## Verification
- `cargo test`: 全80テスト通過（lib 72 + cli 7 + integration 1 [33 fixtures]）
- `cargo clippy`: 警告ゼロ
- `cargo fmt`: フォーマット済み
- 単一ファイルモード: 全既存フィクスチャ通過

## Failed Approaches
なし — 実装はスムーズに完了。

## Next Steps
- Iter 4 未実装: バリデーション強化、戻り値追加の対話フロー
- 対話モード + マルチファイルの統合（現在はエラー）
