# PROGRESS.md

## Current State
- Phase 1-5 + Iter 1-8 完了
- 102 tests, Latest commit: `c225cba`

## Current Tasks: Iter 9 — バグ修正 + コード品質改善

### Task 1: BUG-2 — `block_preview` UTF-8 パニック [バグ]
- `interactive.rs:141` の `&preview[..max_len]` がマルチバイト文字でパニック
- `char_indices` で安全な切断点を求める

### Task 2: BUG-1/5 — `apply_block_edits` + 関数挿入のオフセットずれ [バグ]
- `rewrite.rs:106` — edits のオフセットを `source` から計算するが `result` に適用
- `rewrite.rs:163-176` — block edit 後に `body_start_offset`/`class_def_offset` がずれる可能性
- 現状は「ブロックが重ならない＋後ろから処理」で動くが脆い
- `result` からオフセットを計算するか、edit pass を統合する

### Task 3: DUP-1 — マルチファイル scan+SourcedBlock 構築の3箇所重複 [重複]
- `main.rs`, `interactive.rs`, `tests/integration.rs` で同一パターン
- `lib.rs` に `scan_all_files()` を抽出

### Task 4: ROB-1 — 重複マッチの重なり検出 [堅牢性]
- スライディングウィンドウで重なるマッチが発生しうる
- scanner またはedit適用前に重なり検出/除去

### Task 5: DEAD-1 + DUP-4 — 小さなクリーンアップ [品質]
- `thiserror` 未使用依存を Cargo.toml から削除
- `hash_stmts` / `hash_stmt_refs` をジェネリック関数に統一
- INCON-1: "need >= 2 blocks" エラーメッセージを1箇所に集約

## Completed
- Phase 1-5: 基本機能すべて実装済み
- Phase 6 Iter 1: ビルトイン除外 (`ruff_python_stdlib` 導入) ✅
- Phase 6 Iter 2: スコープ対応配置 ✅
- Phase 6 Iter 3: エッジケーステスト ✅
- Phase 6 Iter 3.5: 兄弟スコープ横断スキャン ✅
- Phase 6 Iter 4: 対話モード ✅
  - パイプライン3段階分割 (`plan_extraction` → `ExtractionPlan` → `apply_refactoring`)
  - `--interactive` (`-i`) CLI フラグ
  - 対話フロー: ブロック選択 → 関数名 → パラメータリネーム → 戻り値リネーム → 戻り値追加 → プレビュー
  - 入力バリデーション（識別子チェック、重複名拒否、生成コード検証）
- Phase 6 Iter 5: 複数ファイル対応 ✅
  - `find_matches_with_hash()` / `find_matches_in_file()` で再帰的スキャン
  - `SourcedBlock` + `plan_extraction_multi()` でクロスファイル計画
  - `apply_refactoring_multi()` でファイルごとの書き換え + `from <stem> import <func>` 挿入
  - CLI: `pym A.py B.py C.py START END [--write] [--diff]`
  - 5つのマルチファイルフィクスチャ
- Phase 6 Iter 6: 対話モード + マルチファイル統合 ✅
  - `run_interactive_multi()` + `select_sourced_blocks()` 追加
  - マルチファイル+対話モードの `bail!` を除去
- Iter 7: 未対応構文の divergence extraction 完全対応 ✅
  - 内包表記, FString/TString, Lambda, Match, FunctionDef, ClassDef, TypeAlias
  - Call keyword 引数の divergence 漏れバグ修正
  - IpyEscapeCommand は対象外（IPython専用）と決定
- Iter 8: スキャン再帰化バグ修正 + リファクタリング ✅
  - `find_matches_with_hash` が子スコープ・親bodyを再帰探索しないバグ修正
  - sibling loop 廃止 → `scan_all_bodies_recursive(search_root)` に統一
  - dead code (`same_body`) 削除、-5行

## Refactoring History
- **スコープ探索統一:** `find_scopes_inner` に統合（-69行）
- **indent計算統一:** `normalize::indent_at_offset` に共通化
- **AST ベース識別子置換:** `replace_names_ast` でピンポイント置換（文字列・コメント誤置換を修正）
- **パイプライン分割:** `plan_extraction` + `apply_refactoring` に分割、AST ボロー問題を解消
- **テスト MECE 改善 (08cc18b):**
  - scan.rs: 冗長テスト5つ → パラメータ化2つに統合
  - scope.rs: builtins/tuple unpacking/del/for-loop/block_stores テスト追加
  - diff_extract.rs: if/for/while/return のネスト body 分岐テスト追加
  - rewrite.rs: `generate_function_def` ユニットテスト追加
- **DRY + 型安全性改善 (c51ae4c):**
  - `apply_block_edits` + `build_call_edits` で重複編集ロジックを共通化
  - `prompt_block_selection` でブロック選択UIを統合
  - `NodePosition { offset, len }` 構造体で `(usize, usize)` タプル8箇所を置換

## Design Decisions
- output は全スコープ統一で after_block 依存（Class 特別扱いなし）
- self.x 代入は return 不要（属性副作用はミュータブル参照経由）
- クロススコープ抽出は親スコープに配置
- 識別子置換は AST ノード位置ベース
- 対話モード: パラメータ/戻り値の「除外」は不要、「追加」のみ
- クロスファイル抽出は `ScopeKind::Module` に強制

## Next Steps
- 新機能のアイデアは design_doc.md を参照
- divergence extraction: IpyEscapeCommand のみ未対応（IPython専用、対象外と決定）

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み
- クラスbodyに self 付きメソッドとして配置: class body に self が存在しないため不可
- Class スコープで全 store を output: 他スコープと不整合。統一ルールに変更
- テキストベース識別子置換: 文字列・コメント内を誤置換。AST ベースに置換
- 対話モードのパラメータ/戻り値「除外」: ユースケースが薄い

## Blockers
(なし)
