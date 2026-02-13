# PROGRESS

AIがPROGRESSを管理するためのもの。PhaseやIter内のProgressを管理するために用いる。

## Current State
- Phase 1-5 + Iter 1-12 完了、バグ修正6件完了
- 59 フィクスチャ PASS、既知バグ 0 件
- fuzz test 106/106 PASS

## Completed

### Phase 1-5: 基盤実装 ✅
AST正規化+ハッシュ、類似ブロックスキャン、変数スコープ解析、コード書き換え+差分出力、CLI改善

### Iter 1-6: 反復改善 ✅
| Iter | 内容 |
|------|------|
| 1 | ビルトイン除外 (`ruff_python_stdlib`) |
| 2 | 抽出先スコープ変更（最小共通スコープ配置） |
| 3+3.5 | スコープテスト + 兄弟スコープ横断スキャン |
| 4 | 対話モード（3段階パイプライン分割: scan → plan → apply） |
| 5 | 複数ファイル対応（`SourcedBlock`, `plan_extraction_multi()`, クロスファイル import） |
| 6 | 対話モード + マルチファイル統合 |

### Iter 7-12: 構文対応・安全性・バグ修正 ✅
| Iter | 内容 |
|------|------|
| 7 | 未対応構文の divergence extraction（内包表記, FString, Lambda, Match, FunctionDef, ClassDef, TypeAlias） |
| 8 | スキャン再帰化 + 制御フロー内ブロックスキャン (if/for/while/with/try/match body) |
| 9 | バグ修正 + コード品質改善 (UTF-8パニック修正, DRY -142行) |
| 10 | 非Exprフィールドのハッシュ・スコープ漏れ修正 (.attr, keyword .arg, is_async, Lambda scope) |
| 11 | 抽出可能性検証 SafetyChecker (break/continue/return/yield) |
| 12 | セマンティックバグ修正 (match_divergence拒否, class scope全store出力) |

### バグ修正 ✅
| バグ | 内容 | コミット |
|------|------|---------|
| 1 | F-string リテラルセグメント parameterization | `443f0f4`, `775bf59`, `179d303` |
| 2 | after_block スコープ境界まで収集 | `303da1d` |
| 3 | f-string リテラルセグメントのハッシュ漏れ | `778ce27` |
| 4 | 同値リテラルの誤パラメータ化（divergent_literal_offsets 導入） | `d65f331` |
| 5 | 3+ブロックで divergent_literal_offsets が block0 vs block1 のみ → 全比較 union | `9e18d90` |
| 6 | 3+ブロックで collect_literal_params が ordinal 照合 → オフセットベース照合 | `9e18d90` |

## Design Decisions
- **Output 判定**: Module/Class → 全 store。Function → after_block で使われる store のみ
- **self.x 代入**: return 不要（属性副作用はミュータブル参照経由）
- **識別子置換**: AST ノード位置ベース + divergent_literal_offsets で非divergentリテラルを保護
- **対話モード**: パラメータ除外不要、戻り値追加のみ有用
- **クロスファイル**: `ScopeKind::Module` に強制、`from X import func` 自動挿入

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み
- クラスbodyに self 付きメソッド配置: class body に self が存在しないため不可
- Class スコープで全 store を output: 過去に不整合として revert → Iter 12 で再導入（必要）
- テキストベース識別子置換: 文字列・コメント内を誤置換 → AST ベースに置換
- テキストベース rename_map でリテラル置換: 同値リテラルの誤置換 → position-based に修正

### 処理フロー監査で発見 (2026-02-13)
| 項目 | 内容 | 状態 |
|------|------|------|
| 二重パース | find_matches と plan_extraction が同じソースを2回パース | 保留（ボトルネックではない） |
| バグ #5 | divergent_literal_offsets が first comparison のみ → union に修正 | ✅ 修正済み |
| バグ #6 | collect_literal_params が ordinal 照合 → offset-based に修正 | ✅ 修正済み |

### コード監査で発見 (2026-02-14)
| バグ | 内容 | 状態 |
|------|------|------|
| #7 | scope.rs: except `as` 変数の Store 漏れ | ✅ `8934805` |
| #8 | scope.rs: match パターン束縛の Store 漏れ | ✅ `8934805` |
| #9 | rewrite.rs: 末尾改行なしファイルで import 挿入位置が範囲外 | ✅ `8934805` |

## Next Steps
- diff_extract.rs: except handler `name` の差分未抽出（影響: except 変数名が異なるブロック間でパラメータ化されない）
- diff_extract.rs: f-string `conversion` フィールド (`!r`,`!s`,`!a`) 未比較（影響: conversion が異なるブロックで bail!）
- 二重パース: API 変更が大きい割にボトルネックではない。将来対応

## Blockers
(なし)
