# PROGRESS.md

## Current State
- Phase 1-5 + Iter 1-9 完了
- 110 tests, Latest commit: `5704ca7`

## Iter 9 完了: バグ修正 + コード品質改善 ✅
- BUG-2: `block_preview` UTF-8 パニック修正 (`char_indices`)
- BUG-1/5: 調査済み → バグではない（debug_assert + コメント追加）
- DUP-1: `scan_all_sources()` 抽出で3箇所重複解消
- ROB-1: 重複マッチの重なり防止（`i += window_size` skip）
- DEAD-1 + DUP-4: `thiserror` 削除、`hash_stmt_iter` 統一
- DUP-2/3: `interactive.rs` リファクタリング（`sync_linked_returns`, `interactive_naming`, `plan_apply` テストヘルパー）

## Current Tasks: Iter 10 — 非Exprフィールドのハッシュ・スコープ漏れ修正

根本原因: `walk_expr`/`walk_stmt` は `Expr`/`Stmt` ノードしか走査しない。
`Identifier`, `bool` 等の非 Expr フィールドがハッシュ・スコープ分析から漏れている。
該当バリアントのみ destructuring で明示処理する（残りは `walk_expr` 委譲のまま）。

### Task 1: normalize.rs — 非 Expr フィールドをハッシュに追加
対象バリアント (destructuring):
- `Expr::Attribute` — `.attr: Identifier` をハッシュ (B1)
- `Expr::Call` — keyword `.arg: Option<Identifier>` をハッシュ (B2)
- `Stmt::For` — `is_async: bool` をハッシュ
- `Stmt::With` — `is_async: bool` をハッシュ
- `Stmt::Try` — `is_star: bool` をハッシュ
- `Comprehension` (ListComp/SetComp/DictComp/Generator 内) — `is_async: bool` をハッシュ

### Task 2: scope.rs — Lambda パラメータのスコープ修正
- `visit_expr` で `Expr::Lambda` を特別処理 (B3)
- パラメータ名を Store として記録してから body を走査
- → lambda 内変数が外部入力として誤検出されなくなる

### Task 3: テスト

#### テスト戦略
パラメータ化テストで各修正を検証。ユニットテストのみ（統合フィクスチャは不要: ハッシュ差異とスコープ判定の単体検証で十分）。

#### normalize.rs テスト (hash が異なることを assert)
| ペア A | ペア B | 検証対象 |
|--------|--------|---------|
| `obj.read()` | `obj.write()` | B1: `.attr` |
| `func(a=1)` | `func(b=1)` | B2: keyword `.arg` |
| `for x in y: pass` | `async for x in y: pass` | `For.is_async` |
| `with ctx(): pass` | `async with ctx(): pass` | `With.is_async` |
| `try: pass\nexcept E: pass` | `try: pass\nexcept* E: pass` | `Try.is_star` |
| `[x for x in y]` | `[x async for x in y]` | `Comprehension.is_async` |

#### scope.rs テスト
| ケース | 期待 | 検証対象 |
|--------|------|---------|
| `lambda x: x + y` | inputs = `[y]` のみ（`x` は含まない） | B3: Lambda パラメータ除外 |
| `lambda x, y=z: x + y` | inputs = `[z]` のみ | B3: default 値は input |

### 終了条件
- [ ] 上記テーブルの全ペアでハッシュが異なること
- [ ] 構造的に等価なペア（変数名/リテラルのみ異なる）はハッシュが一致すること（既存テストで担保）
- [ ] Lambda パラメータが input に含まれないこと
- [ ] 既存 110 テスト全通過
- [ ] `cargo clippy` 警告ゼロ

---

## Next: Iter 11 — 抽出可能性の検証 (SafetyChecker)

詳細は design_doc.md Iter 11 参照。

### テスト戦略
ユニットテスト（`safety.rs` 内パラメータ化）+ 統合フィクスチャ 2 件。

#### ユニットテスト (check_extractable)
| ケース | 期待 | 理由 |
|--------|------|------|
| `break` 直接 | NG | loop_depth == 0 |
| `continue` 直接 | NG | loop_depth == 0 |
| `for x in y: break` | OK | loop_depth > 0 |
| `return x` 直接 | NG | function_depth == 0 |
| `def f(): return x` | OK | function_depth > 0 |
| `lambda: x` 内の `yield` | OK | function_depth > 0 |
| `yield x` 直接 | NG | function_depth == 0 |
| `yield from gen()` 直接 | NG | function_depth == 0 |
| `x = 1; y = x + 2` | OK | フロー文なし |

#### 統合フィクスチャ
- `error_break_not_extractable/` — `break` を含む → `expected_error.txt`
- `error_yield_not_extractable/` — `yield` を含む → `expected_error.txt`

### 終了条件
- [ ] 危険なフロー文を含むブロックがエラーで拒否されること
- [ ] ネストしたループ/関数/lambda 内のフロー文は安全と判定されること
- [ ] 安全なブロックの既存動作が変わらないこと（全既存テスト通過）
- [ ] `cargo clippy` 警告ゼロ

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
- 制御フロー内スキャン: if/for/while/with/try/match body の再帰走査（Iter 11 が前提）
- 新機能のアイデアは design_doc.md を参照

## Failed Approaches
- モジュールスコープ名の自動除外: revert済み
- クラスbodyに self 付きメソッドとして配置: class body に self が存在しないため不可
- Class スコープで全 store を output: 他スコープと不整合。統一ルールに変更
- テキストベース識別子置換: 文字列・コメント内を誤置換。AST ベースに置換
- 対話モードのパラメータ/戻り値「除外」: ユースケースが薄い

## Blockers
(なし)
