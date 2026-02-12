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
- Iter 8: 制御フロー内ブロックスキャン (commit `472912f`)
- Iter 9: SafetyChecker（break/continue/return/yield の安全性検証）
- MECE テスト補完（function_with_output, function_literal_divergence, tuple_unpacking）
- Pedantic clippy 修正

## 現在のタスク: バグ修正セッション

### Bug 1 (HIGH): F-string リテラルセグメントの divergence 未検出

**症状**: `f"Pending: {order_id}"` vs `f"Shipped: {order_id}"` で "Pending"/"Shipped" の差異が検出されず、block 0 のテキストがハードコードされる。

**根本原因**: normalizer と diff_extract の不整合。
- normalizer: `InterpolatedStringElement::Literal` のテキストをハッシュしない → 異なるテキストでも同じハッシュ
- diff_extract: `InterpolatedStringElement::Literal` ペアをスキップ（「normalizer が処理する」というコメントだが嘘）
- 同様に `FStringPart::Literal`（f-string 連結時のプレーン文字列部分）も未処理

**設計判断**: f-string リテラルセグメントはパラメータ化不可能（f-string の AST 構造を変更する必要がある）。
→ `MatchValue`/`MatchMapping` と同じアプローチ: normalizer でリテラルテキストをハッシュし、異なるテキストのブロックをマッチさせない。

**修正方針**:
1. `normalize.rs`: `visit_interpolated_string_element` をオーバーライド → Literal のテキストをハッシュ
2. `normalize.rs`: `visit_string_literal` をオーバーライド → FStringPart::Literal のテキストをハッシュ
3. `diff_extract.rs`: コメント更新（「normalizer がハッシュするため同一保証」）
4. テスト追加

### Bug 2 (MEDIUM): 制御フロー内 after_block が狭すぎる

**症状**: 制御フロー body 内のブロック抽出時、after_block が CF body の後続文のみ。親スコープの後続文が含まれない。
**影響**: 出力変数の検出漏れの可能性（対話モードで手動追加可能）。
**方針**: 既知の制限として記録。将来的にafter_block を親スコープまで拡張する可能性。

### Bug 3 (LOW/doc): onboarding.md のハッシュ表が古い

**症状**: `.attr`, `keyword.arg`, `is_async`, `is_star` が「未実装」と記載されているが、normalize.rs では実装済み。
**修正**: onboarding.md の表を更新。

## Failed Approaches
(なし)

## Blockers
(なし)
