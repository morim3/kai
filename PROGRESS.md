# PROGRESS.md

## Current State
- All phases (1-5) implemented, Phase 6 Iter 1-3 完了
- 37 unit tests + 5 CLI tests + 26 fixtures = all passing
- Latest commit: `1784003`

## Completed
- Phase 1-5: 基本機能すべて実装済み
- Phase 6 Iter 1: ビルトイン除外 (`ruff_python_stdlib` 導入) ✅
- Phase 6 Iter 2: スコープ対応配置 ✅
  - Function → ネスト関数、Class → クラス外に配置、Module → 先頭
  - output 判定は全スコープ統一（after_block 依存、Class 特別扱いなし）
- Phase 6 Iter 3: エッジケーステスト ✅
  - 9 パターンの fixture 追加（method_in_class, nested_function,
    class_in_function, deep_nesting, async_function, class_with_after_code,
    self_attr, blank_lines, cross_function_error）

## Design Decisions
- **output は全スコープ統一で after_block 依存:**
  Class スコープも特別扱いしない。対話モード（Iter 4）で手動追加可能。
- **self.x 代入は return 不要:**
  属性への副作用はミュータブル参照経由で反映される。
- **モジュールスコープ名の自動除外は行わない:**
  将来のCLI制御（パラメータ手動選択）に委ねる。

## Next Step
- **Iter 3.5: 兄弟スコープ横断スキャン**
  同一親スコープ内の兄弟 body（他の関数/クラスの body）も横断スキャン。
  変更箇所: scan.rs（兄弟 body スキャン）、lib.rs（per-block after_block 算出）

## Iteration Plan
- Iter 3.5: 兄弟スコープ横断スキャン ← 次
- Iter 4: 対話モード（配置位置選択含む）
- Iter 5: 複数ファイル対応

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み。
- クラスbodyに self 付きメソッドとして配置: class body に self が存在しないため不可。
- Class スコープで全 store を output: 他スコープと不整合。統一ルールに変更。

## Blockers
(なし)
