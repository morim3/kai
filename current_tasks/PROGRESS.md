# PROGRESS

## 完了済み

- Phase 1-5: 基盤実装（正規化、スキャン、スコープ解析、書き換え、CLI）
- Iter 1: ビルトイン除外
- Iter 2: 抽出先スコープ変更（最小共通スコープ配置）
- Iter 3+3.5: スコープテスト + 兄弟スコープ横断スキャン
- Iter 4: 対話モード（3段階パイプライン分割）
- Iter 5: 複数ファイル対応
- Iter 6: 対話 + マルチファイル統合
- Iter 7: 未対応構文の divergence extraction（内包表記, FString, Lambda, Match 等）
- Iter 8: 制御フロー内ブロックスキャン + after_block スコープ境界修正
- Iter 9: SafetyChecker（break/continue/return/yield の安全性検証）
- MECE テスト補完（function_with_output, function_literal_divergence, tuple_unpacking）
- Pedantic clippy 修正
- Bug 1: F-string リテラルセグメント parameterization (`443f0f4`, `775bf59`, `179d303`)
- Bug 2: after_block スコープ境界まで収集 (`303da1d`)
- Bug 3: onboarding.md ハッシュ表更新 (`778ce27`)

## 現在のタスク — Fuzz Testing バグ修正

fuzz test 60件中 58 PASS、2 FAIL

### Bug 4: 同値リテラルの誤パラメータ化 (fuzz 035)
- `val ** 2` vs `num ** 2` — べき指数 `2` は両ブロック同じなのにパラメータ化される
- Fixture: `tests/fixtures/power_same_exponent/`
- 原因調査先: `diff_extract.rs`

### Bug 5: self.attr 名の誤リネーム (fuzz 070)
- `self.x`/`self.y` が `self.w`/`self.area` に変わる
- Fixture: `tests/fixtures/self_attr_rename_bug/`
- 原因調査先: `diff_extract.rs` / `rewrite.rs`

## 全テスト状況
55 + 2 = 57 フィクスチャ (55 PASS, 2 FAIL — Bug 4, Bug 5)
