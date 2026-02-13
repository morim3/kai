# PROGRESS.md

AIがPROGRESSを管理するためのもの。PhaseやIter内のProgressを管理するために用いる。

## Current State
- Phase 1-5 + Iter 1-12 完了
- 117 tests, Latest commit: `aab5c21`

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
- Iter 9: バグ修正 + コード品質改善 ✅
  - UTF-8 パニック修正、重複マッチ防止、DRY 改善 (-142行)
  - `interactive.rs` リファクタリング: 共有ヘルパー抽出、テスト重複排除
- Iter 10: 非Exprフィールドのハッシュ・スコープ漏れ修正 ✅
  - normalize.rs: `.attr`, keyword `.arg`, `is_async`, `is_star` をハッシュに追加
  - scope.rs: Lambda パラメータのスコープ修正（ネスト VarCollector）
  - フィクスチャ更新: `self_attr`（同一属性名に変更）, `lambda_divergence`（パラメータ数修正）
- Iter 11: 抽出可能性の検証 (SafetyChecker) ✅
  - `src/safety.rs` 新規: `SafetyChecker` + `check_extractable()` + `format_unsafe_error()`
  - `lib.rs`: `plan_extraction_multi()` 冒頭で block 0 の安全性検証
  - break/continue (loop_depth), return/yield/yield_from (function_depth) の深さ追跡
  - 統合フィクスチャ: `error_return_not_extractable`, `error_yield_not_extractable`
- Iter 12: セマンティックバグ修正（手動トレース監査で発見）✅
  - **match_divergence修正**: MatchValue/MatchMapping keyパターン内のdivergenceを拒否
    - `case 1:` → `case arg_0:` でvalue patternがcapture patternに変わる問題
    - `diff_patterns` で divergence検出時にbail（エラーフィクスチャに変換）
    - `match_safe_divergence` フィクスチャ追加（subject/body divergenceは安全）
  - **class scope出力修正**: クラスbody内のstore全てをoutputとして扱う
    - `analyze_block` に `all_stores_as_outputs` フラグ追加
    - `unify_signatures` → `plan_extraction_multi` 経由で `ScopeKind::Class` 判定
    - 関数が `return ret_0, ret_1` を返し、呼び出し側で `x, y = extracted_func_0(...)` に変更
    - 3つのクラスフィクスチャ更新: `class_method`, `class_with_after_code`, `class_in_function`

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
- output は基本的に after_block 依存。ただし Class スコープでは全 store を output（クラス属性保持のため）
- self.x 代入は return 不要（属性副作用はミュータブル参照経由）
- クロススコープ抽出は親スコープに配置
- 識別子置換は AST ノード位置ベース
- 対話モード: パラメータ/戻り値の「除外」は不要、「追加」のみ
- クロスファイル抽出は `ScopeKind::Module` に強制

## Next Steps
- 制御フロー内スキャン: if/for/while/with/try/match body の再帰走査（Iter 11 が前提）
- 新機能のアイデアは design_doc.md を参照

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み
- クラスbodyに self 付きメソッドとして配置: class body に self が存在しないため不可
- Class スコープで全 store を output: 過去に不整合として revert → Iter 12 で再導入（クラス属性消失バグのため必要）
- テキストベース識別子置換: 文字列・コメント内を誤置換。AST ベースに置換
- 対話モードのパラメータ/戻り値「除外」: ユースケースが薄い

## Blockers
(なし)
