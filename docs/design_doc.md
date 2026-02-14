
# Design Doc: Python Extract Method Refactoring Tool (Rust)

人間とAIが作るべきもの、作り方についての理解を深めるためのドキュメント。
プロジェクト全体に関係する文章については、消去されることは原則としてない。
一方で機能追加に伴って更新されることは考えられる。
また、Iteration管理は新しいiterに進むたびに、古いIterationは整理されるべきである。
直前, 直後のiterは詳しく書き、過去/未来になればなるほどのiterの内容は簡潔にされるべきである。
iterationの中身は背景、内容と終了条件で構成される。
iterationはfeatureの実装のほかにもbugfixであってもいい。終了条件がきちんと定義されていればいい。
終わったiterationを整理するときにわかりやすさのためにそれをsquashしてもよい。
詳細が確定して実装された内容については、整理されonboardingなどに移されるべきである。基本的にcodeが真のドキュメントであり、それをわかりやすくするためにonboardingがある。



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

#### ハッシュ判断基準

各ASTノードの各フィールドについて、以下の3ルールで「ハッシュする/正規化する/そのままハッシュする」を判断する。

**Rule 1: 操作の種類はハッシュする**
文/式のkind（discriminant tag）、演算子、ExprContext、boolフラグ（`is_async`, `is_star`, `simple`, `parenthesized`, `level`）など、「何をしているか」を決定する要素はすべてハッシュする。

**Rule 2: ブロック内のローカル束縛は正規化する**
ブロック内で定義・束縛される識別子（変数名、for-loop target、except handler name、関数/クラス名）は、出現順の位置IDに正規化する。リテラル値は一律 `CONSTANT` に正規化する。これらは抽出時にパラメータ化可能な要素。

**Rule 3: 外部参照の識別子はハッシュする**
ブロック外で定義されたAPIへの参照（`Attribute.attr`、`Keyword.arg`、import のモジュール名・インポート名）は、そのままハッシュする。変えたら別の操作になるため。

**判定テスト: 「この識別子はブロック内で定義されたものか、外部で定義されたものか？」**
- 内部定義 → Rule 2（正規化）
- 外部参照 → Rule 3（ハッシュ）

**例外: 正規化すると意味論が変わるもの → ソーステキストをそのままハッシュ**
- `MatchValue`: `case 1:` の `1` をパラメータ化すると `case x:` となり、値マッチからキャプチャパターンに意味が変わる。
- `MatchMapping` keys: 同上。キーはリテラルまたはドット名でなければならない。

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

### 完了済み: Iter 1-12 (反復改善) ✅

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
| 8 | 制御フロー内ブロックスキャン | if/for/while/with/try/match body に再帰。`collect_after_stmts()` でスコープ境界まで後続文収集 |
| 9 | SafetyChecker | `safety.rs`: break/continue/return/yield の安全性検証。ネストした loop/function 内は安全 |
| 10 | 非Exprフィールド修正 | `.attr`, `keyword.arg`, `is_async`, Lambda スコープのハッシュ・差分・スコープ漏れ修正 |
| 11 | 抽出可能性検証統合 | `plan_extraction_multi()` 冒頭で `check_extractable()` 呼び出し |
| 12 | セマンティックバグ修正 | `match_divergence` 拒否、class scope 全 store 出力 |

### 完了済みの主要な設計判断

- **Output 判定**: Module/Class スコープでは ALL stores を output（グローバル変数/クラス属性は外部参照可能）。Function スコープでは after_block で使われる store のみ。`target_block_scope`（配置スコープではなく）で判定。
- **`self.x = ...` は return 不要**: 属性への副作用。ミュータブル参照経由で反映。
- **パラメータ除外ステップ不要**: block 0 の値がハードコードされるだけで有用性なし。
- **戻り値追加は有用**: after_block 解析で未検出の変数を対話モードで手動追加可能。
- **クロスファイル時は Module 配置強制**: 関数定義はターゲットファイルに、他ファイルは `from X import func`。
- **after_block 収集**: `collect_after_stmts()` が制御フローを再帰降下し、各ネストレベルの後続文をスコープ境界まで収集。
- **divergent_literal_offsets**: 同値リテラルの誤パラメータ化を防ぐため、position-based で divergent リテラルを追跡。
- **SafetyChecker**: block 0 のみ検証で十分（全ブロックは同一構造）。エラー時は bail。
