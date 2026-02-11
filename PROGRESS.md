# PROGRESS.md

## Current State
- All phases (1-5) implemented, Phase 6 Iter 1-3.5 完了
- リファクタリングセッション完了
- 40 unit tests + 5 CLI tests + 28 fixtures = all passing
- Latest commit: `32125a8`

## Completed
- Phase 1-5: 基本機能すべて実装済み
- Phase 6 Iter 1: ビルトイン除外 (`ruff_python_stdlib` 導入) ✅
- Phase 6 Iter 2: スコープ対応配置 ✅
  - Function → ネスト関数、Class → クラス外に配置、Module → 先頭
  - output 判定は全スコープ統一（after_block 依存、Class 特別扱いなし）
- Phase 6 Iter 3: エッジケーステスト ✅
  - 9 パターンの fixture 追加
- Phase 6 Iter 3.5: 兄弟スコープ横断スキャン ✅
  - `find_scopes` で innermost と parent を1回の探索で取得
  - `find_body_for_block` で各ブロックの body を個別に取得、per-block after_block 算出
  - `find_scope_for_matches` でクロススコープ時は親スコープコンテキストを使用
  - cross_function, cross_function_with_output fixture 追加

## Refactoring (Tech Debt Reduction)
- **スコープ探索統一:** `find_innermost_body_inner` + `find_parent_with_ctx_inner` (4関数 119行)
  → `find_scopes_inner` (1関数 50行) に統合。1回の探索で innermost/parent 両方を返す。
- **indent計算統一:** `indent_of_body` (scan.rs) + `indent_at_offset` (rewrite.rs)
  → `indent_at_offset` を normalize.rs に共通化。
- **AST ベース識別子置換:** `replace_identifier`（テキストベース単語境界マッチ）
  → `replace_names_ast`（Visitor で Expr::Name/Literal の TextRange を収集しピンポイント置換）。
  文字列リテラル・コメント内の誤置換バグを修正。

## Design Decisions
- **output は全スコープ統一で after_block 依存:**
  Class スコープも特別扱いしない。対話モード（Iter 4）で手動追加可能。
- **self.x 代入は return 不要:**
  属性への副作用はミュータブル参照経由で反映される。
- **モジュールスコープ名の自動除外は行わない:**
  将来のCLI制御（パラメータ手動選択）に委ねる。
- **クロススコープ抽出は親スコープに配置:**
  兄弟関数/クラスにまたがるマッチは共通の親スコープに関数を配置。
- **識別子置換は AST ノード位置ベース:**
  テキストマッチではなく、パーサーが識別した Name/Literal ノードの正確なバイト範囲のみ置換。

## Next Step
- **Iter 4: 対話モード（配置位置選択含む）**

## Iteration Plan
- Iter 4: 対話モード（配置位置選択含む）← 次
- Iter 5: 複数ファイル対応

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み。
- クラスbodyに self 付きメソッドとして配置: class body に self が存在しないため不可。
- Class スコープで全 store を output: 他スコープと不整合。統一ルールに変更。
- テキストベース識別子置換 (`replace_identifier`): 文字列・コメント内を誤置換。AST ベースに置換。

## Blockers
(なし)
