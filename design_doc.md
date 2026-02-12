
# Design Doc: Python Extract Method Refactoring Tool (Rust)

## 1. Overview (概要)
* **目的:** ユーザーが指定したPythonのコード片（リファレンスブロック）を基準として、同一ファイル内から「構造的に一致する」他のコードブロックを検索し、それらをまとめて1つの共通関数として抽出・置換するリファクタリングツールをRustで開発する。
* **アプローチ:** AST（抽象構文木）の正規化比較とデータフロー解析による、決定論的なコード変換。

## 2. Scope (スコープ)
### In-Scope (実装するもの)
* ユーザーからのCLI入力受け付け（対象ファイルパス、基準となるコードの開始行・終了行）。
* ASTに基づくコードの正規化（変数名やリテラルの違いを無視した構造の抽象化）。
* 正規化されたASTハッシュを用いた、ファイル内からの類似ブロックのスキャンとマッチング。
* 抽出対象ブロック群における、外部依存変数（引数）と再代入変数（戻り値）の特定（スコープ・生存変数解析）。
* 共通関数の定義コード生成と、元の各コードブロックの関数呼び出しへの置換パッチ生成。
* 元のコードのインデントやフォーマットを破壊しない書き換え（差分出力またはファイル上書き）。

### Out-of-Scope (実装しないもの)
* LLMや外部APIを用いた命名生成（関数名や引数名は `extracted_func`, `arg_0`, `arg_1` のようなプレースホルダーとする）。
* ~~複数ファイルにまたがるコードの検索および置換~~ → Iter 5 で実装済み。
* `eval()`, `exec()`, `locals()`, `globals()` などを含む動的で解析困難なコードブロックの抽出。
* 型推論やクラス継承関係を考慮した意味的マッチング。
* IPython/Jupyter 専用構文（`IpyEscapeCommand`）を含むコードブロックの抽出。

## 3. Architecture & Core Crates (アーキテクチャと利用クレート)
Rustの堅牢なエコシステム、特にPython解析のデファクトスタンダードとなりつつあるRuffの基盤を活用する。



* **パーサー & AST操作 (`ruff_python_parser`, `ruff_python_ast`, `ruff_text_size`):**
    * Pythonコードから型安全なASTを構築し、`Visitor` トレイトを用いてノードを巡回する。`ruff_text_size` を用いて、元のソースコードのバイトオフセットを正確に追跡し、非破壊的な書き換え（Edit）を実現する。
* **ハッシュ計算 (`rustc-hash`):**
    * ASTノードの構造的正規化ハッシュを高速に計算する。暗号論的安全性は不要なため、スキャンスピードに優れる `FxHash` を使用する。
* **差分生成・パッチ適用 (`similar`):**
    * リファクタリング前後のテキスト差分（Unified Diff）を生成し、ユーザーのコンソールに表示する。
* **CLI & エラーハンドリング (`clap`, `anyhow`, `thiserror`):**
    * CLI引数の型安全なパースと、ファイルI/Oやパース失敗時の透過的なエラーハンドリングを行う。

## 4. Core Algorithms (中核アルゴリズム)

### A. 構造的正規化とハッシュ化 (Structural Normalization)
`ruff_python_ast::Visitor` を実装し、指定されたコード片のASTをトラバースする。



* `Expr::Name` (変数参照/代入) → 実際の変数名を無視し、出現順序に基づく相対的なタグ（例: `VAR_0`, `VAR_1`）に置き換えてハッシュ化。
* `Expr::Constant` (リテラル等) → 実際の値を無視し、一律 `CONSTANT` トークンとしてハッシュ化。
* これにより、「変数の使われ方の構造」が同じであれば、同一のハッシュ値が算出される。

### B. スコープ解析とインターフェース抽出 (Scope Analysis)
抽出対象となる複数のブロックに対してデータフロー解析を行い、関数のインターフェースを決定する。



1. **Inputs (引数):** ブロック内で `Load` されているが、その前にブロック内で `Store` されていない変数のセット。正規化で差異が出たリテラルや変数も引数として昇格させる。
2. **Outputs (戻り値):** ブロック内で `Store` されており、かつブロック終了後の後続コードで `Load` される変数のセット。

## 5. Implementation Phases & Exit Criteria (実装フェーズと終了条件)

### 完了済み: Phase 1-5 (基盤実装) ✅

| Phase | タスク | 概要 |
|-------|--------|------|
| 1 | AST正規化+ハッシュ化 | 変数名・リテラルを消した構造的ハッシュで等価性判定 |
| 2 | 類似ブロックスキャン | スライディングウィンドウで同一ハッシュのブロックを全探索 |
| 3 | 変数スコープ解析 | データフロー解析で引数(inputs)と戻り値(outputs)を特定 |
| 4 | コード書き換え+差分出力 | 関数定義生成 + ブロック→呼び出し置換 + unified diff |
| 5 | CLI改善 | `--diff`, `--write`, `--name` オプション |

### 完了済み: Iter 1-7 (反復改善) ✅

| Iter | タスク | 主な設計判断 |
|------|--------|------------|
| 1 | ビルトイン除外 | `ruff_python_stdlib::is_python_builtin()` で静的除外。import名は除外しない（再代入可能） |
| 2 | 抽出先スコープ変更 | 最小共通スコープに配置。Class body → クラス外（self 不使用）。Output は全スコープ統一 after_block 依存 |
| 3+3.5 | スコープテスト+横断スキャン | 兄弟スコープ（別関数 body）も横断スキャン。`find_scopes` で innermost+parent を1回探索 |
| DRY | リファクタリング | AST ベース識別子置換（`replace_names_ast`）。indent計算共通化。スコープ探索統合 |
| 4 | 対話モード | 3段階パイプライン分割（scan → plan → apply）。パラメータ除外不要、戻り値追加が有用 |
| 5 | 複数ファイル対応 | `SourcedBlock`, `plan_extraction_multi()`。クロスファイル → Module 配置強制 |
| 6 | 対話+マルチファイル統合 | `run_interactive_multi()`。ファイル名付きブロック選択 |
| 7 | 未対応構文の divergence | 内包表記, FString/TString, Lambda, Match, FunctionDef, ClassDef, TypeAlias |

### 完了済みの主要な設計判断

- **Output 判定**: Module/Class スコープでは ALL stores を output（グローバル変数/クラス属性は外部参照可能）。Function スコープでは after_block で使われる store のみ。
- **`self.x = ...` は return 不要**: 属性への副作用。ミュータブル参照経由で反映。
- **パラメータ除外ステップ不要**: block 0 の値がハードコードされるだけで有用性なし。
- **戻り値追加は有用**: after_block 解析で未検出の変数を対話モードで手動追加可能。
- **クロスファイル時は Module 配置強制**: 関数定義はターゲットファイルに、他ファイルは `from X import func`。

---

### Iter 8: 制御フロー内ブロックスキャン

* **背景:** 現在の `scan_all_bodies_recursive` は `FunctionDef` と `ClassDef` の body のみに再帰する。
  `if`/`for`/`while`/`with`/`try`/`match` の内部にあるブロックは探索対象外。

  ```python
  def process(items):
      for item in items:
          # このブロックは現在見つからない
          x = item.value
          result = x * 2
          print(result)
  ```

* **方針:** 制御フロー文の body にも再帰的にスキャンを拡張する。
  制御フロー文は Python のスコープを作らないため、スコープ判定（`ScopeKind`）は変更不要。

* **実装:**

  1. **`scan_all_bodies_recursive`** (`scan.rs`): 以下の match arm を追加

     | Stmt | 再帰対象の body |
     |------|----------------|
     | `If` | `.body`, `.elif_else_clauses[*].body` |
     | `For` | `.body`, `.orelse` |
     | `While` | `.body`, `.orelse` |
     | `With` | `.body` |
     | `Try` | `.body`, `.handlers[*].body`, `.orelse`, `.finalbody` |
     | `Match` | `.cases[*].body` |

  2. **`find_scopes_inner`** (`scan.rs`): 同じ再帰パターンを追加。
     制御フロー内部に入っても `ScopeKind` は変わらない（Python のスコープではない）。
     再帰で `child` を返す際は `current_kind` をそのまま引き継ぐ。

  3. **`find_body_for_block`** (`scan.rs`): `find_innermost_body` → `find_scopes` 経由で
     制御フロー内のブロックが所属する body を正しく返す必要がある。

  4. **After-block 計算の課題:**
     制御フロー内のブロックの after_block は「同一 body 内の後続文」のみ。
     親スコープの後続文は含まれないため、output 検出が conservative になる場合がある。

     ```python
     def process():
         if True:
             x = 1        # block
             y = x + 2    # block
         print(y)         # 親スコープ → after_block に含まれない
     ```

     対策案:
     - **A. Conservative (初期実装):** 同一 body 内の後続文のみ。output が不足する場合は対話モードで手動追加。
     - **B. Walk-up (将来):** 制御フロー body の後続文 + 親スコープ body の後続文を連結。

* **テスト (フィクスチャ — 制御フロー全種類):**

  | フィクスチャ | カバー対象 |
  |-------------|-----------|
  | `if_body_scan` | If body + elif/else body |
  | `for_body_scan` | For body |
  | `while_body_scan` | While body |
  | `with_body_scan` | With body |
  | `try_body_scan` | Try handler body |
  | `match_body_scan` | Match case body |
  | `nested_control_flow` | 制御フロー内制御フロー |

* **修正対象ファイル:**

  | ファイル | 変更内容 |
  |---------|---------|
  | `src/scan.rs` | `scan_all_bodies_recursive`, `find_scopes_inner`, `find_body_for_block` に制御フロー再帰追加 |
  | `tests/fixtures/` | 新規フィクスチャ 7 件 |

* **Exit Criteria:**
  * 全制御フロー文（if, for, while, with, try, match）内のブロックが検出・抽出されること。
  * ネストした制御フロー内のブロックも検出されること。
  * スコープ判定が正しいこと（制御フローで ScopeKind が変わらない）。
  * 既存テスト全通過。

---

### Iter 9: 抽出可能性の検証 (SafetyChecker) ✅
* **背景:** 現在のツールは、指定ブロックが関数に抽出可能かどうかを検証しない。
  ブロック内に特定のフロー制御文が含まれると、抽出後にコードが壊れる:

  | フロー文 | 問題 | 壊れる例 |
  |---------|------|---------|
  | `break` | 関数内で SyntaxError | `for x in items: break` → `def f(): break` |
  | `continue` | 関数内で SyntaxError | `while True: continue` → `def f(): continue` |
  | `return` | 意味が変わる | 呼び出し元関数からの脱出 → 抽出関数からの脱出に |
  | `yield` / `yield from` | 意味が変わる | ジェネレータの yield が普通の関数に移動 |

  ただし、ブロック内のネストされたスコープ内にあるフロー文は安全:
  ```python
  # 安全: break はブロック内の for 自身を対象
  for x in items:
      if bad(x):
          break            # ← depth > 0 なので OK

  # 安全: return はネストした lambda/関数を対象
  fn = lambda x: x + 1     # ← function_depth > 0 なので OK
  ```

* **方針:** 新モジュール `src/safety.rs` に `SafetyChecker` を実装し、
  `plan_extraction_multi()` の冒頭で block 0 に対して検証を実行する。
  安全でないブロックにはエラーを返す。

* **実装:**
  1. **`src/safety.rs`**: `SafetyChecker` 構造体 + `check_extractable()` 公開関数。
     ```rust
     struct SafetyChecker {
         loop_depth: usize,      // Stmt::For / Stmt::While に入ると +1
         function_depth: usize,  // Stmt::FunctionDef / Expr::Lambda に入ると +1
         unsafe_nodes: Vec<UnsafeNode>,
     }

     struct UnsafeNode {
         kind: UnsafeKind,  // Break, Continue, Return, Yield
         offset: usize,     // ソース上のバイト位置（エラーメッセージ用）
     }

     /// ブロックの文を走査し、安全でないフロー文を収集する。
     pub fn check_extractable(stmts: &[Stmt]) -> Result<(), Vec<UnsafeNode>>;
     ```
  2. **判定ルール:**
     - `Stmt::Break` / `Stmt::Continue` → `loop_depth == 0 && function_depth == 0` なら NG
     - `Stmt::Return` → `function_depth == 0` なら NG
     - `Expr::Yield` / `Expr::YieldFrom` → `function_depth == 0` なら NG
  3. **Visitor 実装:**
     - `visit_stmt`: `For` / `While` で `loop_depth += 1` → body 走査 → `loop_depth -= 1`
     - `visit_stmt`: `FunctionDef` で `function_depth += 1` → body 走査 → `function_depth -= 1`
     - `visit_expr`: `Lambda` で `function_depth += 1` → body 走査 → `function_depth -= 1`
     - `visit_stmt`: `Break` / `Continue` / `Return` で depth 判定 → NG なら `unsafe_nodes` に追加
     - `visit_expr`: `Yield` / `YieldFrom` で depth 判定 → NG なら追加
     - 他の Stmt/Expr は `walk_stmt` / `walk_expr` に委譲
  4. **パイプライン統合 (`lib.rs`):**
     - `plan_extraction_multi()` の冒頭、scope_ctx 決定前に block 0 の Stmt に対して
       `check_extractable()` を呼ぶ。Err なら人間可読なエラーメッセージで `bail!`。
     - エラーメッセージ例: `"Cannot extract: block contains 'break' at line 5 (not inside a loop within the block)"`
  5. **テスト:**
     - ユニットテスト (`safety.rs` 内):
       - `break`/`continue` が直接 → NG
       - `break` がブロック内の `for` 内 → OK
       - `return` が直接 → NG
       - `return` がネスト関数/lambda 内 → OK
       - `yield` が直接 → NG
       - `yield` がネスト関数内 → OK
       - 安全なブロック → OK
     - 統合フィクスチャ:
       - `tests/fixtures/error_break_not_extractable/`: `break` を含むブロック → `expected_error.txt`
       - `tests/fixtures/error_yield_not_extractable/`: `yield` を含むブロック → `expected_error.txt`

* **設計判断:**
  - block 0 のみ検証で十分: 全ブロックは同一構造なので、block 0 が安全なら全ブロック安全。
  - エラー時は bail（警告ではなくエラー）: 壊れたコードの生成を防ぐ。
  - `async for` / `async with` は `loop_depth` / 通常走査で処理（`await` は安全なため対象外）。

* **修正対象ファイル:**

  | ファイル | 変更内容 |
  |---------|---------|
  | `src/safety.rs` | 新規: `SafetyChecker` + `check_extractable()` |
  | `src/lib.rs` | `pub mod safety;` 追加 + `plan_extraction_multi()` に検証呼び出し |
  | `tests/fixtures/error_break_not_extractable/` | 新規フィクスチャ |
  | `tests/fixtures/error_yield_not_extractable/` | 新規フィクスチャ |

* **Exit Criteria:**
  * `break`/`continue` を含むブロックの抽出が明確なエラーメッセージで拒否されること。
  * `return`/`yield` を含むブロックの抽出が拒否されること。
  * ネストした for/while/関数/lambda 内のフロー文は安全と判定されること。
  * 安全なブロックの既存動作が変わらないこと（全既存テスト通過）。
