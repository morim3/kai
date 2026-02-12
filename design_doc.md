
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

### Phase 1: AST正規化とターゲットブロックのハッシュ化
* **タスク:** CLI引数（`file.py`, `start_line`, `end_line`）を受け取り、対象範囲のコードを `ruff_python_parser` でパース。`Visitor` を用いて正規化ハッシュを計算する。
* **Exit Criteria:**
  * コマンド実行時にパースエラーでクラッシュしない。
  * 変数名やリテラルだけが異なる等価な2つのコード片を入力した際、生成される「正規化ハッシュ値」が完全に一致するテストがパスすること。

### Phase 2: 同一ファイル内の類似ブロックスキャン
* **タスク:** Phase 1のハッシュ計算ロジックをファイル全体の各ステートメント群に適用（スライディングウィンドウ等）し、ターゲットとハッシュが一致するブロックをすべて見つけ出す。
* **Exit Criteria:**
  * 対象ファイル内にターゲットと構造が同じブロックが複数存在する場合、それらすべての「開始行・終了行（またはバイトオフセット）」が標準出力にリストアップされること。

### Phase 3: 変数スコープ解析（引数・戻り値の特定）
* **タスク:** マッチした各コードブロックのASTを解析し、外部から渡すべき変数（引数）と、外部へ返す変数（戻り値）を特定する。また、各ブロック間の変数の対応関係を解決する。
* **Exit Criteria:**
  * 抽出ブロックを入力として、必要な `inputs` (引数リスト) と `outputs` (戻り値リスト) を正確に返す構造体・関数が実装され、テストがパスすること。
  * 異なる変数が使われているブロック同士を比較し、共通関数のシグネチャが破綻なく生成できること。

### Phase 4: コードの書き換えと差分出力 (Rewriting)
* **タスク:** 新しい共通関数（`def extracted_func_0(arg_0, arg_1):`）のAST/文字列を生成し、マッチした元の各ブロックを関数呼び出しに置き換える。
* **Exit Criteria:**
  * 置換後のコードが有効なPythonコードとしてパース可能（SyntaxErrorにならない）であること。
  * `similar` クレートを用いて、リファクタリング前後のUnified Diffが標準出力されること。
  * `--write` オプションを付与した場合、元のコードのインデントを維持したままファイルが正しく上書き保存されること。

### Phase 5: CLIの使いやすさ向上 (Usability Improvements) ✅
* **タスク:** コマンドラインツールとしての実用性を高める。
  1. **デフォルト出力の変更:** `--write` なしの場合、リファクタリング後のソースコードを標準出力に出力。`--diff` で Unified Diff。
  2. **命名カスタマイズ機能:** `--name` で関数名を指定可能。
* **Exit Criteria:**
  * デフォルトで標準出力にリファクタリング後のコード全文が出力されること。
  * `--name my_func` のように指定した場合、`def my_func(arg_0, arg_1):` として関数が生成されること。

### Phase 6: スコープとパラメータ制御の改善

#### Iter 1: ビルトイン除外 ✅
* **タスク:** `ruff_python_stdlib` を依存に追加し、ハードコードのビルトインリストを `ruff_python_stdlib::builtins::is_python_builtin()` に置換する。
* **Exit Criteria:**
  * `print`, `range`, `len` 等のビルトインがパラメータにならないこと。
  * Python 3.10+ のビルトイン (`aiter`, `anext`) も正しく除外されること。
* **設計判断:** import名やモジュールスコープ変数の自動除外は行わない。
  理由: 関数名・import名もPythonでは再代入可能であり、「何を除外すべきか」の判断は
  自動化すると条件分岐が増える。将来のCLI制御（パラメータ手動選択）に委ねる方がシンプル。

#### Iter 2: 抽出先スコープの変更（最深配置） ✅
* **方針:** 抽出した関数を、マッチブロック群の**最小共通スコープ**に配置する。
* **実装結果:**
  1. 同一関数内のブロック → その関数内にネスト関数として配置。
  2. 同一クラスbody内のブロック → クラスの**外**に通常関数として配置（`self` 不使用）。
  3. トップレベルのブロック → モジュールレベルに配置（既存動作維持）。
* **設計判断:**
  * クラスbody直下では `self` が存在しないため、クラス外の通常関数として配置。
  * output の判定は全スコープ統一で after_block 依存（Class 特別扱いなし）。
  * `self.x = ...` は属性への副作用であり return 不要（ミュータブル参照経由で反映）。

#### Iter 3: エッジケーステスト（スコープ極限テスト） ✅
* **テスト対象と結果:**
  1. クラスメソッド内（Class → Function） ✅
  2. ネスト関数（Function → Function） ✅
  3. 関数内クラス（Function → Class） ✅
  4. 深いネスト（Function → Class → Method） ✅
  5. async 関数 ✅
  6. クラスbody + 後続コード ✅
  7. self.x 属性代入 ✅
  8. 空行で分離されたブロック ✅
  9. 別関数に同パターン → エラー（同一スコープ内のみ検索）
* **発見した課題:** 兄弟スコープ（別関数の body）を横断スキャンしていない。
  → Iter 3.5 で修正。

#### Iter 3.5: 兄弟スコープ横断スキャン ✅
* **方針:** 同一親スコープ内の兄弟 body（他の関数/クラスの body）も横断的にスキャンし、
  スコープをまたぐ重複ブロックを検出する。
* **実装結果:**
  1. `scan.rs`: `find_scopes` で innermost と parent を1回の探索で取得（統合済み）。
  2. `scan.rs`: `find_body_for_block` で各マッチブロックの所属 body を特定。
  3. `scan.rs`: `find_scope_for_matches` でクロススコープ時は親スコープコンテキストを使用。
  4. `lib.rs`: per-block after_block 算出（各ブロックの所属 body から取得）。
* **Exit Criteria:**
  * `def foo(): a=1; b=a+2` と `def bar(): x=10; y=x+20` が同一関数に抽出されること。✅
  * 同一スコープ内のマッチが引き続き正しく動作すること。✅

#### リファクタリング: DRY & バグ修正 ✅
* **スコープ探索統一:** `find_innermost_body_inner` + `find_parent_with_ctx_inner`
  → 単一の `find_scopes_inner` に統合。1回の探索で innermost/parent 両方を返す（-69行）。
* **indent計算統一:** scan.rs / rewrite.rs の重複 → `normalize::indent_at_offset` に共通化。
* **AST ベース識別子置換:** `replace_identifier`（テキストベース）は文字列リテラル・コメント内の
  識別子を誤置換するバグあり。`Visitor` で `Expr::Name` / `Expr::*Literal` の `TextRange` を収集し
  ピンポイント置換する `replace_names_ast` に置換。

#### Iter 4: 対話モード ✅
* **方針:** デフォルトは現在と同じ自動モード。`--interactive` (`-i`) で対話モードに切り替え。
* **アーキテクチャ:** パイプラインを3段階に分割し、AST ボロー問題を解消。
  * `scan::find_matches()` → `Vec<MatchedBlock>` (Stage 1)
  * `plan_extraction()` → `ExtractionPlan` (Stage 2: owned data, AST borrow 不要)
  * `rewrite::apply_refactoring()` → `String` (Stage 3)
  * `ExtractionPlan` = `{ sig, scope_ctx, ref_node_positions: Vec<NodePosition> }`
  * `collect_node_positions()` で AST ノード位置を事前収集 → ステージ間で owned data として引き回し
* **対話フロー:**
  1. ブロック選択（MultiSelect: どのマッチを置き換えるか）
  2. 関数名入力（Input + バリデーション）
  3. パラメータリネーム（Input × 各パラメータ + バリデーション）
  4. 戻り値リネーム（Input × 各戻り値 + バリデーション）
  5. 戻り値追加（ブロック内 store 変数から選択）
  6. プレビュー＋書き込み確認（Confirm）
* **設計判断（パラメータ/戻り値の「除外」は不要）:**
  * パラメータ除外: block 0 の値がハードコードされるだけで有用なユースケースがない。
  * 戻り値除外: 後続コードの変数が未定義になるだけ。
  * 代わりに「戻り値の追加」が有用: after_block 解析で検出されなかった変数を
    手動で返り値に追加する（遠くで使われる変数、リファクタ後に使いたい変数）。
* **入力バリデーション:**
  * 有効な Python 識別子チェック（`is_valid_python_ident` / `validate_ident`）✅
  * パラメータ名の重複チェック（`rename_collection` 内）✅
  * 生成結果を `ruff_python_parser::parse_module` で検証（`validate_output`）✅
  * rename map の整合性検証（`validate_rename_map`: 衝突・マージ・重複チェック）✅
* **Exit Criteria:**
  * `--interactive` なしでは現在と同じ出力であること。✅
  * 対話モードでブロック選択・関数名・パラメータ名・戻り値名をカスタマイズできること。✅
  * どんなユーザー入力でも SyntaxError が出力されないこと。✅
  * 戻り値を手動追加できること。✅

#### Iter 5: 複数ファイル対応 ✅
* **方針:** 複数ファイルにまたがる構造的に同一のブロックを検出し、共通関数として抽出する。
* **実装結果:**
  1. `scan.rs`: `find_matches_with_hash()` でハッシュ+window_size+マッチを返す新API追加。
     `find_matches_in_file()` で任意ソースを再帰的にスキャン（全スコープ対応）。
  2. `lib.rs`: `SourcedBlock`（MatchedBlock + source_index）と `plan_extraction_multi()` 追加。
     クロスファイル時は `ScopeKind::Module` に強制。既存 `plan_extraction` はラッパー化。
  3. `rewrite.rs`: `generate_import()`, `apply_refactoring_multi()` 追加。
     ターゲットファイルに関数定義配置、他ファイルに `from <stem> import <func>` 挿入。
  4. `main.rs`: CLI を `pym A.py B.py C.py START END [--write] [--diff]` 形式に拡張。
     1ファイルは既存動作と完全互換。
* **テスト:** 5つのマルチファイルフィクスチャ追加（multi_simple, multi_with_returns,
  multi_inside_function, multi_three_files, multi_no_match_in_extra）。全33フィクスチャ通過。
* **Exit Criteria:**
  * 複数ファイルに同一構造のブロックがある場合、共通関数に抽出され、各ファイルから正しくimportされること。✅
  * 単一ファイルモードの動作が変わらないこと。✅

#### Iter 6: 対話モード + マルチファイル統合 ✅
* **方針:** `--interactive` と複数ファイル指定を組み合わせ可能にする。
* **実装結果:**
  1. `interactive.rs`: `run_interactive_multi()` 追加。`select_sourced_blocks()` でファイル名付きブロック選択。
  2. `main.rs`: マルチファイル+対話モードの `bail!` を除去、`run_interactive_multi` にルーティング。
  3. プレビュー・書き込みはファイルごとに表示/確認。
* **Exit Criteria:**
  * `pym a.py b.py 1 2 -i` で対話的にブロック選択・リネーム・プレビューができること。✅
  * 非対話モードの動作が変わらないこと。✅

#### Iter 7: 未対応構文の divergence extraction 対応 ✅
* **方針:** `diff_extract.rs` で not-implemented エラーを返していた構文のうち、実用頻度の高いものを実装する。
* **実装結果:**
  1. **内包表記** (ListComp, SetComp, DictComp, Generator): `diff_comprehensions()` ヘルパーで `target`, `iter`, `ifs` を再帰的に diff。
  2. **FString / TString**: `diff_interpolated_elements()` ヘルパーで `Interpolation` 内の式と `format_spec` を再帰的に diff。
  3. **Lambda**: `diff_parameters()` ヘルパーでパラメータ名と default 値を diff。`diff_param_names()` で Identifier の Name divergence を手動処理。
  4. **Match**: `diff_patterns()` 関数で Pattern の 8 バリアント全てを再帰的に diff。subject, guard, body も diff。
  5. **FunctionDef**: name, decorator_list, parameters (`diff_parameters` 再利用), returns, body を diff。
  6. **ClassDef**: name, decorator_list, base classes (arguments), body を diff。
  7. **バグ修正**: `Expr::Call` で keyword 引数の value が diff されていなかった問題を修正。
  8. **TypeAlias**: name, value を diff。
  9. IpyEscapeCommand は非サポート（IPython/Jupyter 専用構文のため対象外）。
* **テスト:** ユニットテスト 17 件追加、統合フィクスチャ 4 件追加（comprehension, fstring, lambda, match）。全38フィクスチャ通過。
* **Exit Criteria:**
  * 内包表記、f-string、lambda、match、FunctionDef、ClassDef を含むブロックで正しくパラメータ化されること。✅
  * Call の keyword 引数の divergence が正しく抽出されること。✅
  * 既存テストが全て通過すること。✅

#### Iter 11: 抽出可能性の検証 (SafetyChecker)
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
