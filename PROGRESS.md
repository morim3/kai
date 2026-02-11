# PROGRESS.md

## Current State
- All phases (1-5) implemented, Phase 6 Iter 1-2 完了
- 38 unit tests + 5 CLI tests + 17 fixtures = all passing
- Latest commit: `859c643`

## Completed
- Phase 1-5: 基本機能すべて実装済み
- Phase 6 Iter 1: ビルトイン除外 (`ruff_python_stdlib` 導入) ✅
- Phase 6 Iter 2: スコープ対応配置 ✅
  - Function スコープ → ネスト関数として body 先頭に配置
  - Class スコープ → クラス外に通常関数として配置、全 store を return
  - Module スコープ → ファイル先頭に配置（既存動作維持）

## Design Decisions
- **モジュールスコープ名の自動除外は行わない:**
  import名・関数名もPythonでは再代入可能であり、自動除外すると条件分岐が増える。
  将来のCLI制御（`--exclude-params`）でユーザーが明示的に制御する方がシンプル。
- **クラスbodyは self を使わない:**
  クラスbody直下に self は存在しない。クラス外の通常関数として配置し、
  全 store を output として return する。
- **Module スコープの output は after_block 依存のまま:**
  全 store を output にすると既存テストが大量に壊れる。
  Module レベルで変数が失われるケースは稀であり、対話モード（Iter 4）で対応可能。

## Next Step
- **Iter 3: エッジケーステスト** — スコープの極限パターンを検証
  - クラスメソッド内、ネスト関数、関数内クラス、深いネスト、async 関数

## Iteration Plan
- Iter 3: エッジケーステスト（スコープ極限テスト）
- Iter 4: 対話モード（配置位置選択含む、旧 Iter 3 + 旧 Iter 4 統合）
- Iter 5: 複数ファイル対応

## Failed Approaches
- モジュールスコープ名の自動除外: import/def/classのみ除外したが、
  再代入の可能性や、将来CLI制御を入れることを考えるとシンプルさに欠ける。revert済み。
- クラスbodyに self 付きメソッドとして配置: class body 直下に self が存在しないため不可。

## Blockers
(なし)
