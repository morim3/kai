# kai オンボーディングガイド

## kai は何をするツール？

Python コードの中から **構造的に同じパターンの繰り返し** を自動で見つけ出し、
関数に抽出するリファクタリングツール。Rust で書かれている。

### Before

```python
a = 1
b = a + 2
print(b)

c = 10
d = c + 20
print(d)

e = 100
f = e + 200
print(f)
```

### After (`kai example.py 1 3` を実行)

```python
def extracted_func_0(arg_0, arg_1):
    a = arg_0
    b = a + arg_1
    print(b)

extracted_func_0(1, 2)
extracted_func_0(10, 20)
extracted_func_0(100, 200)
```

ユーザーが指定するのは **ファイル名と行範囲だけ**。
ツールが自動的に「同じパターン」を探し、違いをパラメータ化する。

---

## パイプライン概要

処理は3段階で進む。各段階の実装箇所をコード参照付きで示す。

```
入力: ソースコード + 行範囲
       ↓
┌─────────────────────────────────────────────────────┐
│ Stage 1: スキャン                                     │
│  scan.rs    — スライディングウィンドウでマッチ探索      │
│  normalize.rs — AST を正規化してハッシュ               │
└──────────────┬──────────────────────────────────────┘
       ↓ Vec<MatchedBlock>
┌─────────────────────────────────────────────────────┐
│ Stage 2: 計画                                        │
│  lib.rs         — パイプライン全体を調整               │
│  scope.rs       — 入出力変数の特定                     │
│  diff_extract.rs — ブロック間の差分抽出                 │
└──────────────┬──────────────────────────────────────┘
       ↓ ExtractionPlan
┌─────────────────────────────────────────────────────┐
│ Stage 3: 適用                                        │
│  rewrite.rs — 関数定義生成 + ブロック→呼び出し置換     │
└─────────────────────────────────────────────────────┘
       ↓
出力: リファクタリング済みソースコード
```

全体の流れは `lib.rs` の2つの関数で読める:
- **ワンショット**: `extract_method_with_options()` (`lib.rs:261`) — scan → plan → apply を一気に実行
- **段階分割**: `plan_extraction_multi()` (`lib.rs:132`) → `rewrite::apply_refactoring()` (`rewrite.rs:147`)

---

## Stage 1: スキャン — 「同じ構造」をどう見つけるか

### 構造的ハッシュ (`normalize.rs`)

Python コードを AST（抽象構文木）に変換し、**変数名とリテラル値を消した状態** でハッシュを計算する。

```python
a = 1 + 2       # AST: Assign(Store(VAR_0), BinOp(CONST, Add, CONST))
x = 10 + 20     # AST: Assign(Store(VAR_0), BinOp(CONST, Add, CONST))
```

変数名 `a`/`x` は `VAR_0` に、リテラル `1`/`10` は `CONST` に正規化される。
結果として **同じハッシュ値** になり、「構造的に等価」と判定される。

一方、演算子が違えば別のハッシュになる。

```python
a = 1 + 2       # BinOp(CONST, Add, CONST)  → ハッシュ A
a = 1 - 2       # BinOp(CONST, Sub, CONST)  → ハッシュ B (違う!)
```

**実装箇所:**
- `NormalizeVisitor` (`normalize.rs:83`) — AST を走査してハッシュを蓄積する Visitor
- `visit_expr()` (`normalize.rs:165`) — 式ノードの処理。`Expr::Name` は位置 ID に、リテラルは `CONSTANT` に正規化。それ以外は種類タグ + `walk_expr` で再帰
- `visit_stmt()` (`normalize.rs:132`) — 文ノードの処理。種類タグをハッシュして `walk_stmt` で再帰
- `visit_operator()` / `visit_cmp_op()` 等 (`normalize.rs:243-283`) — 演算子をハッシュ
- `hash_block()` (`normalize.rs:30`) — 外部向け API: ソース + 行範囲 → ハッシュ値

### ruff Visitor が提供するもの vs 手動で追加が必要なもの

ruff の `Visitor` トレイトは **AST のツリーノード型** に対する `visit_*` メソッドを提供する。
しかし、`Identifier`（文字列）や `bool` などのノードでないフィールドは走査対象外であり、
ハッシュに含めるには各ノードのハンドラで **明示的にフィールドを読む** 必要がある。

| ruff Visitor が提供 | 用途 |
|---|---|
| `visit_stmt` / `walk_stmt` | 文ノードの再帰。子 Stmt/Expr を自動走査 |
| `visit_expr` / `walk_expr` | 式ノードの再帰。子 Expr を自動走査 |
| `visit_bool_op`, `visit_operator`, `visit_unary_op`, `visit_cmp_op` | 演算子のハッシュ |
| `visit_expr_context` | Load/Store/Del の区別 |
| `visit_comprehension` / `walk_comprehension` | 内包表記の target, iter, ifs を走査 |
| `visit_except_handler`, `visit_pattern`, ... | その他のノード型 |

| **Visitor が走査しない** (手動で追加が必要) | 型 | 理由 |
|---|---|---|
| `Attribute.attr` | `Identifier` | 葉の属性値でありノードではない |
| `Keyword.arg` | `Option<Identifier>` | 同上 |
| `Parameter.name` | `Identifier` | 同上 |
| `For.is_async`, `With.is_async` | `bool` | ノードではない |
| `Comprehension.is_async` | `bool` | 同上 |
| `Try.is_star` | `bool` | 同上 |

### 正規化されるもの / されないもの

| 分類 | 例 | ハッシュに含まれる？ | 実装箇所 | 状態 |
|------|-----|:---:|------|:---:|
| 文の種類 | `Assign`, `For`, `If` | Yes | `visit_stmt` :134 | 実装済 |
| 式の種類 | `BinOp`, `Call` | Yes | `visit_expr` :188 | 実装済 |
| 演算子 | `+`, `-`, `<`, `>` | Yes | `visit_operator` :243 | 実装済 |
| ExprContext | Load, Store, Del | Yes | `visit_expr_context` :230 | 実装済 |
| 変数名 | `x`, `data` | **No** (正規化) | `visit_expr` :168 | 実装済 |
| リテラル値 | `1`, `"hello"` | **No** (正規化) | `visit_expr` :177 | 実装済 |
| 属性名 | `.read`, `.write` | Yes | `visit_expr` Attribute arm | 実装済 |
| キーワード引数名 | `key=`, `value=` | Yes | `visit_keyword` | 実装済 |
| `is_async` | `async for` vs `for` | Yes | `visit_comprehension` / `visit_stmt` | 実装済 |
| `is_star` | `except*` vs `except` | Yes | `visit_except_handler` | 実装済 |
| f-string リテラル | `"hello "` in f-string | Yes | `visit_interpolated_string_element` | 実装済 |

### スライディングウィンドウ (`scan.rs`)

ユーザーが指定した行範囲のブロックサイズを「ウィンドウ」として、
ファイル全体をスライドしながら同じハッシュのブロックを探す。

**ウィンドウの単位は「AST の Stmt ノード数」** であり、行数ではない。
`select_stmts()` (`normalize.rs:46`) が行範囲にかかる Stmt を選び、その個数が `window_size` になる。
複数行にまたがる `if` や `for` も1つの Stmt として数える。

```python
a = 1              # Stmt 1 (Assign)
b = a + 2          # Stmt 2 (Assign)
if x > 0:          # Stmt 3 (If) ← 3行あるが 1 Stmt ( 現状 )
    y = x + 1
    print(y)
```

ユーザーが行 1-2 を指定 → `window_size = 2` → 連続2つの Stmt ずつスライド:

```
body = [Stmt1, Stmt2, Stmt3]

[Stmt1, Stmt2] → ハッシュ計算 → ターゲットと比較
[Stmt2, Stmt3] → ハッシュ計算 → ターゲットと比較
```

**実装箇所:**
- `find_matches_with_hash()` (`scan.rs`) — ターゲットブロックのハッシュを計算し、同じファイル内のマッチを全スコープから探す
- `scan_body_with_hash()` (`scan.rs`) — 単一 body 内のスライディングウィンドウ (`body[i..i+window_size]`)。マッチ後は `window_size` だけスキップして重なりを防ぐ
- `find_matches_in_file()` (`scan.rs`) — 別ファイル用: 既知のハッシュ+ウィンドウサイズで全スコープを走査
- `scan_all_sources()` (`lib.rs:93`) — ターゲット + 追加ファイルをまとめてスキャンし `Vec<SourcedBlock>` を返す

### 再帰スキャン (`scan_all_bodies_recursive`)

スライディングウィンドウは1つの `&[Stmt]` (body) に対して動作する。
ファイル全体を探索するには、各スコープの body に対して再帰的にスキャンする必要がある。

現在の実装は **`FunctionDef` と `ClassDef` の body のみ** に再帰する:

```rust
// scan.rs — scan_all_bodies_recursive()
fn scan_all_bodies_recursive(source, body, target_hash, window_size, matches) {
    scan_body_with_hash(source, body, ...);  // この body 自体をスキャン
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(f) => scan_all_bodies_recursive(..., &f.body, ...),
            Stmt::ClassDef(c)    => scan_all_bodies_recursive(..., &c.body, ...),
            _ => {}  // 現状はif, for, while, with, try の内部には入らない. 将来的には拡張が必要
        }
    }
}
```

**制御フロー文の内部にも再帰する（Iter 8 で実装済み）。**

| Stmt | 内部の body | 状態 |
|------|-----------|:---:|
| `FunctionDef` | `.body` | 実装済 |
| `ClassDef` | `.body` | 実装済 |
| `If` | `.body`, `.elif_else_clauses[*].body` | 実装済 |
| `For` | `.body`, `.orelse` | 実装済 |
| `While` | `.body`, `.orelse` | 実装済 |
| `With` | `.body` | 実装済 |
| `Try` | `.body`, `.handlers[*].body`, `.orelse`, `.finalbody` | 実装済 |
| `Match` | `.cases[*].body` | 実装済 |

実装例:
```rust
Stmt::If(if_stmt) => {
    scan_all_bodies_recursive(..., &if_stmt.body, ...);
    for clause in &if_stmt.elif_else_clauses {
        scan_all_bodies_recursive(..., &clause.body, ...);
    }
}
Stmt::For(for_stmt) => {
    scan_all_bodies_recursive(..., &for_stmt.body, ...);
    if !for_stmt.orelse.is_empty() {
        scan_all_bodies_recursive(..., &for_stmt.orelse, ...);
    }
}
// While, With, Try, Match も同様
```

ただし、制御フロー内のブロックを抽出するには **抽出可能性の検証** が別途必要になる
（後述「制約と未実装機能」を参照）。

### スコープ検出 (`scan.rs`)

マッチしたブロックがどのスコープ（モジュール/関数/クラス）にあるかを判定し、
抽出した関数の配置先を決定する。

**実装箇所:**
- `ScopeKind` / `ScopeContext` (`scan.rs:10-29`) — スコープの種類と配置情報
- `find_scopes()` (`scan.rs:41`) — AST を再帰走査して最内スコープ + 親スコープを返す
- `find_scope_for_matches()` (`scan.rs`) — 全マッチが同一 body なら最内スコープ、異なれば親スコープを返す

---

## Stage 2: 計画 — 何をパラメータにするか

オーケストレーションは `plan_extraction_multi()` (`lib.rs:132`)。
以下の3つの分析を順に実行する。

### 2a. スコープ分析 (`scope.rs`)

各ブロックについて、**入力**（ブロック外で定義されブロック内で使われる変数）と
**出力**（ブロック内で定義されブロック後で使われる変数）を特定する。

```python
# ブロック外
data = load()
                     # ← data は「入力」(Load が先)
result = data + 1    # ← result は「出力」(Store 後にブロック外で使用)
                     #
print(result)        # ← ブロック後で result を使用
```

**実装箇所:**
- `VarCollector` (`scope.rs:229`) — AST を走査して (変数名, Load/Store) イベントを記録
  - `visit_expr()` (`scope.rs:282`) — `Expr::Name` を検出して Load/Store を記録。それ以外は `walk_expr` で再帰
  - `visit_stmt()` (`scope.rs:295`) — `Assign` は RHS → LHS の順に走査（`a = a + 1` で Load を先に記録するため）。`AugAssign` は Load + Store の両方を記録
- `analyze_block()` (`scope.rs:44`) — ブロックと after_block を走査して `BlockInterface { inputs, outputs }` を返す
- `inputs()` (`scope.rs:246`) — Store より先に Load された変数（ビルトイン除外）
- `stores()` (`scope.rs:269`) — ブロック内で Store された全変数

### 2b. 差分抽出 (`diff_extract.rs`)

マッチした複数ブロックを並列走査し、**具体的にどこが違うか** を特定する。

```python
# ブロック A              # ブロック B
a = 1                     c = 10         # Name: a vs c, Literal: 1 vs 10
b = a + 2                 d = c + 20     # Name: b vs d, a vs c, Literal: 2 vs 20
```

**実装箇所:**
- `extract_divergences()` (`diff_extract.rs:27`) — 2つのブロックの文を zip して `diff_stmts` を呼ぶ
- `diff_stmts()` (`diff_extract.rs:40`) — 文の種類ごとに分岐。各フィールドの子 Expr を `diff_exprs` で再帰比較
- `diff_exprs()` (`diff_extract.rs`) — 式の種類ごとに分岐:
  - `Expr::Name`: 名前が違えば `Divergence::Name` を記録
  - リテラル: ソーステキストが違えば `Divergence::Literal` を記録
  - それ以外: 子 Expr を再帰比較
- `Divergence` (`diff_extract.rs:10`) — `Name(String, String)` または `Literal(String, String)`

### 2c. シグネチャ統合 (`scope.rs`)

入力・出力・差分を統合して、抽出する関数のシグネチャを決定する。

```python
def extracted_func_0(arg_0, arg_1):  # arg_0 = a/c (Name入力), arg_1 = 1/10 (Literal)
    ...
    return result  # 出力変数があれば return
```

**実装箇所:**
- `unify_signatures()` (`scope.rs:161`) — 全ブロックの BlockInterface + Divergence から `FunctionSignature` を構築
- `collect_literal_params()` (`scope.rs:119`) — Literal divergence をブロック横断の値テーブルに変換
- `FunctionSignature` (`scope.rs:82`) — パラメータ名、戻り値名、ブロックごとの引数マッピング
- `rename_map()` (`scope.rs:98`) — ブロック0 の元の変数名 → 新しいパラメータ/戻り値名のマッピング

### 計画の出力: `ExtractionPlan` (`lib.rs:67`)

```rust
struct ExtractionPlan {
    sig: FunctionSignature,              // 関数シグネチャ
    scope_ctx: ScopeContext,             // 配置先スコープ情報
    ref_node_positions: Vec<NodePosition>, // ブロック0のAST名前/リテラル位置
    block_stores: Vec<Vec<String>>,      // ブロックごとの全Store変数
}
```

`ref_node_positions` は `rewrite::collect_node_positions()` で収集される。
AST ノードのバイト位置を保存しておくことで、Stage 3 で変数名を正確に置換できる。

---

## Stage 3: 適用 — コードを書き換える

### 関数定義の生成

`generate_function_def()` (`rewrite.rs:28`) がブロック0のソーステキストから関数定義を生成する。

1. ブロック0のソーステキストを抽出 (`:36`)
2. `rename_map()` で元の変数名→パラメータ名の対応を取得 (`:47`)
3. `replace_names_ast()` で AST ノード位置ベースの置換 (`:51`) — 文字列リテラルやコメント内の同名文字列を壊さない
4. `reindent()` でインデントを調整 (`:71`)
5. `def` 行 + body + `return` 文を組み立て (`:74-80`)

### ブロック→呼び出し置換

`generate_call()` (`rewrite.rs:88`) が各ブロックの引数を組み立てる。
`block_arg_maps[block_index]` からブロック固有の値を取り出す。

```python
# ブロック A の引数: extracted_func_0(1, 2)   ← block_arg_maps[0] = ["1", "2"]
# ブロック B の引数: extracted_func_0(10, 20)  ← block_arg_maps[1] = ["10", "20"]
```

### 編集の適用

`apply_block_edits()` (`rewrite.rs:111`) が全ブロックを関数呼び出しに置換する。
**末尾から先頭の順** に処理することで、先の編集が後の編集のオフセットを壊さない。

`apply_refactoring()` (`rewrite.rs:147`) がこれを統合:
1. 関数定義を生成
2. 全ブロックを呼び出しに置換（末尾から）
3. 関数定義をスコープに応じた位置に挿入

### 配置ルール (`rewrite.rs:173-203`)

| ブロックの場所 | 関数の配置先 | 実装 |
|--------------|------------|------|
| モジュール直下 | ファイル先頭に prepend | `:174-177` |
| 関数内 | `body_start_offset` 直前に挿入 | `:178-189` |
| クラス内 | `class_def_offset` 直前に挿入 | `:191-202` |
| 複数ファイル | モジュール直下 + `from X import func` | `apply_refactoring_multi()` :263 |

---

## ファイル構成

```
src/
├── main.rs           # CLI (clap)。引数パース → scan_all_sources → plan → apply
├── lib.rs            # パイプラインオーケストレーション
│   ├── parse_python()           :54  # ruff パーサーのラッパー
│   ├── scan_all_sources()       :93  # Stage 0: 全ファイルスキャン
│   ├── plan_extraction_multi()  :132 # Stage 1+2: 計画
│   ├── plan_extraction()        :237 # ↑の単一ファイル版ラッパー
│   └── extract_method()         :256 # ワンショット API
│
├── normalize.rs      # AST 正規化 + 構造的ハッシュ
│   ├── NormalizeVisitor         :83  # ハッシュ蓄積ビジター
│   ├── hash_block()             :30  # ソース+行範囲→ハッシュ
│   ├── select_stmts()           :46  # 行範囲で文をフィルタ
│   ├── line_of_offset()         :63  # バイトオフセット→行番号
│   └── indent_at_offset()       :73  # オフセット位置のインデント取得
│
├── scan.rs           # スライディングウィンドウ + スコープ検出
│   ├── ScopeKind / ScopeContext :10  # スコープ情報
│   ├── find_matches_with_hash() :    # ターゲットファイルスキャン
│   ├── find_matches_in_file()   :    # 追加ファイルスキャン
│   ├── scan_all_bodies_recursive():  # 全スコープ再帰走査
│   ├── find_scopes()            :41  # 最内+親スコープ検出
│   └── find_scope_for_matches() :    # マッチ群から配置スコープ決定
│
├── scope.rs          # 変数スコープ分析
│   ├── VarCollector             :229 # Load/Store イベント収集
│   ├── analyze_block()          :44  # 入力+出力を特定
│   ├── unify_signatures()       :161 # 全ブロック統合→FunctionSignature
│   └── FunctionSignature        :82  # パラメータ/戻り値/マッピング
│
├── diff_extract.rs   # ブロック間差分抽出
│   ├── Divergence               :10  # Name | Literal
│   ├── extract_divergences()    :27  # 2ブロック比較のエントリポイント
│   ├── diff_stmts()             :40  # 文レベル分岐
│   └── diff_exprs()             :    # 式レベル分岐（再帰）
│
├── rewrite.rs        # コード生成 + 置換
│   ├── NodePosition             :15  # AST ノードのバイト位置
│   ├── generate_function_def()  :28  # 関数定義テキスト生成
│   ├── generate_call()          :88  # 関数呼び出しテキスト生成
│   ├── apply_block_edits()      :111 # 末尾→先頭の順でテキスト置換
│   ├── apply_refactoring()      :147 # 単一ファイル適用
│   └── apply_refactoring_multi():263 # マルチファイル適用
│
├── safety.rs         # 抽出可能性の検証
│   └── check_extractable()      # break/continue/return/yield の安全性チェック
│
└── interactive.rs    # 対話モード (dialoguer)
    ├── run_interactive()         # 単一ファイル対話フロー
    ├── run_interactive_multi()   # マルチファイル対話フロー
    ├── interactive_naming()      # 共通命名フロー (Steps 2-5)
    └── sync_linked_returns()     # パラメータ→戻り値の自動同期

tests/
├── integration.rs    # フィクスチャベースの統合テスト (auto-discover)
├── cli.rs            # CLI テスト (assert_cmd)
└── fixtures/         # テストデータ
    ├── {name}/input.py          # 先頭行: # kai: START-END
    ├── {name}/expected.py       # 期待出力
    ├── {name}/options.toml      # (任意) func_name = "compute"
    └── {name}/known_bug.txt     # (任意) 既知バグとしてスキップ
```

---

## 重要な型

```rust
// scan.rs
struct MatchedBlock {
    start_offset: usize,   // ソース内のバイト開始位置
    end_offset: usize,     // バイト終了位置
    start_line: usize,     // 1-based 開始行
    end_line: usize,       // 1-based 終了行
}

// lib.rs:82
struct SourcedBlock {
    block: MatchedBlock,
    source_index: usize,   // 0=ターゲット, 1+=追加ファイル
}

// scope.rs:82
struct FunctionSignature {
    params: Vec<String>,                // ["arg_0", "arg_1"]
    returns: Vec<String>,               // ["ret_0"] or ["arg_0"] (入力=出力の場合)
    block_arg_maps: Vec<Vec<String>>,   // [["a", "1"], ["c", "10"]]
    block_return_maps: Vec<Vec<String>>,// [["b"], ["d"]]
}

// diff_extract.rs:10
enum Divergence {
    Name(String, String),     // 変数名 "a" vs "c"
    Literal(String, String),  // リテラル値 "1" vs "10"
}

// lib.rs:67
struct ExtractionPlan {
    sig: FunctionSignature,
    scope_ctx: ScopeContext,              // 配置先スコープ
    ref_node_positions: Vec<NodePosition>,// ブロック0のASTノード位置
    block_stores: Vec<Vec<String>>,       // 対話モード用: 追加戻り値候補
}

// rewrite.rs:15
struct NodePosition {
    offset: usize,  // バイトオフセット
    len: usize,     // バイト長
}
```

---

## Visitor パターンと既知の落とし穴

このツールは ruff の AST Visitor パターンを3箇所で使っている:

| 用途 | Visitor | 場所 |
|------|---------|------|
| 構造的ハッシュ | `NormalizeVisitor` | `normalize.rs:83` |
| 差分抽出 | `diff_stmts` / `diff_exprs` (手動走査) | `diff_extract.rs:40` |
| スコープ分析 | `VarCollector` | `scope.rs:229` |

### `walk_expr` の限界

`walk_expr(visitor, expr)` は式の **子 `Expr` ノード** を再帰的に訪問するが、
**`Expr` 型でないフィールドは訪問しない**。

例: `obj.read()` の AST

```
Expr::Attribute
├── value: Expr::Name("obj")      ← walk_expr が訪問する (Expr型)
├── attr: Identifier("read")      ← walk_expr が訪問しない (Identifier型)
├── ctx: ExprContext::Load         ← visit_expr_context で別途処理
└── range: TextRange              ← 位置情報、ハッシュ不要
```

`attr` は `Identifier` 型（ただの文字列ラッパー）なので `walk_expr` の走査対象外。
明示的にハッシュしなければ、`obj.read()` と `obj.write()` が同じハッシュになってしまう。

### 3モジュールそれぞれへの影響

**normalize.rs** — ハッシュに含まれない → 構造が違うブロックが誤マッチ

```python
data.keys()   # ハッシュ = X
data.values()  # ハッシュ = X (同じ! .attr が無視されるため)
```

**diff_extract.rs** — 差分として検出されない → ブロック0の値がハードコード

```python
# 抽出結果: 全ブロックで .keys() が使われてしまう
def extracted_func_0():
    items = data.keys()  # ← .values() のブロックでも .keys() になる
```

**scope.rs** — ローカル変数が外部入力扱い → 不要なパラメータ + 未定義変数

```python
# lambda x: x + 1 → x が外部入力扱いに
def extracted_func_0(arg_0, arg_1):  # arg_0 = x (不要!)
    fn = lambda x: arg_0 + arg_1    # lambda 内で arg_0 は未定義
```

### 同じ問題が起きるフィールド一覧

| バリアント | フィールド | 型 | 影響モジュール |
|-----------|----------|------|------|
| `Attribute` | `.attr` | `Identifier` | normalize, diff_extract |
| `Call` keyword | `.arg` | `Option<Identifier>` | normalize, diff_extract |
| `Lambda` | `parameters` | `Parameters` | scope |
| `For` | `is_async` | `bool` | normalize |
| `With` | `is_async` | `bool` | normalize |
| `Try` | `is_star` | `bool` | normalize |
| `Comprehension` | `is_async` | `bool` | normalize |

### 対策: 該当バリアントのみ destructuring

全フィールドを明示するので、ruff がフィールド追加したらコンパイルエラーになる。

```rust
// Before (normalize.rs): .attr が見えない
Expr::Attribute(_) => "Attribute",  // catch-all で walk_expr に委譲

// After: 全フィールドが見える。attr を使わなければ未使用変数警告
Expr::Attribute(ExprAttribute { value, attr, ctx: _, range: _ }) => {
    self.hash_tag("Attribute");
    self.hash_tag(attr.as_str());
    self.visit_expr(value);
}
```

---

## 制約と未実装機能

詳細は以下のドキュメントを参照:
- **抽出可能性の検証** (`SafetyChecker`): `design_doc.md` Iter 9 (実装済み)
- **制御フロー内スキャン**: `design_doc.md` Iter 8 (次のタスク)
- **ハッシュ漏れ** (`.attr`, `is_async` 等): 本ドキュメントの「正規化されるもの / されないもの」テーブル
- **Lambda スコープ**: 本ドキュメントの「Visitor パターンと既知の落とし穴」セクション
- **今後のタスク**: `PROGRESS.md` の Next Steps

---

## 開発ワークフロー

```bash
# ビルド
cargo build

# テスト実行
cargo test

# Lint
cargo clippy

# 単一ファイルで実行（対話モード = デフォルト）
cargo run -- example.py 1 3

# 非対話モードで実行
cargo run -- example.py 1 3 --no-interactive

# diff 出力
cargo run -- example.py 1 3 --no-interactive --diff

# マルチファイル
cargo run -- main.py utils.py 1 3 --no-interactive

# ファイル直接書き換え
cargo run -- example.py 1 3 --no-interactive --write
```

### テストの追加方法

`tests/fixtures/` にディレクトリを作成するだけで自動検出される (`tests/integration.rs:207`):

```
tests/fixtures/my_test_case/
├── input.py          # 先頭行に `# kai: 1-3` (対象行範囲)
├── expected.py       # 期待される出力
└── options.toml      # (任意) func_name = "compute"
```

エラーケースのテスト:

```
tests/fixtures/my_error_case/
├── input.py          # 先頭行に `# kai: 1-3`
└── expected_error.txt  # エラーメッセージの部分文字列
```

既知バグのマーク:

```
tests/fixtures/known_issue/
├── input.py
├── expected.py
└── known_bug.txt     # 1行目にバグの説明。テスト失敗しても PASS 扱い
```
