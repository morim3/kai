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
- Iter 9: SafetyChecker（break/continue/return/yield の安全性検証）
- MECE テスト補完（function_with_output, function_literal_divergence, tuple_unpacking）
- Pedantic clippy 修正

## 現在のタスク: Iter 8 — 制御フロー内ブロックスキャン

### 概要
`scan_all_bodies_recursive` は現在 `FunctionDef`/`ClassDef` のみに再帰。
`if`/`for`/`while`/`with`/`try`/`match` の body 内のブロックは検出されない。

### 実装ステップ

1. **`scan_all_bodies_recursive`** に制御フロー match arm 追加
   - If: `.body` + `.elif_else_clauses[*].body`
   - For: `.body` + `.orelse`
   - While: `.body` + `.orelse`
   - With: `.body`
   - Try: `.body` + `.handlers[*].body` + `.orelse` + `.finalbody`
   - Match: `.cases[*].body`

2. **`find_scopes_inner`** に同じ再帰パターン追加
   - 制御フローは Python スコープを作らない → `ScopeKind` は変更しない
   - `body_start_offset` / `indent` は制御フロー body のものを使う

3. **After-block 計算の確認**
   - `find_body_for_block` が制御フロー body を正しく返すか検証
   - Conservative: 同一 body 内の後続文のみ（不足分は対話モードで手動追加）

4. **テストフィクスチャ追加** (制御フロー全種類)

   | フィクスチャ | カバー対象 |
   |-------------|-----------|
   | `if_body_scan` | If `.body` + `.elif_else_clauses[*].body` |
   | `for_body_scan` | For `.body` |
   | `while_body_scan` | While `.body` |
   | `with_body_scan` | With `.body` |
   | `try_body_scan` | Try `.handlers[*].body` |
   | `match_body_scan` | Match `.cases[*].body` |
   | `nested_control_flow` | 制御フロー内制御フロー |

### Exit Criteria
- [ ] `if` 内の重複ブロックが検出・抽出される（elif/else body 含む）
- [ ] `for` 内の重複ブロックが検出・抽出される
- [ ] `while` 内の重複ブロックが検出・抽出される
- [ ] `with` 内の重複ブロックが検出・抽出される
- [ ] `try` の handler 内の重複ブロックが検出・抽出される
- [ ] `match` の case 内の重複ブロックが検出・抽出される
- [ ] ネストした制御フロー内のブロックが検出される
- [ ] スコープ判定が正しい（制御フローで ScopeKind が変わらない）
- [ ] 既存テスト全通過
- [ ] clippy 警告ゼロ

## Failed Approaches
(なし)

## Blockers
(なし)
