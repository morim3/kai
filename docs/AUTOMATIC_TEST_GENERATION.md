# Automatic Fixture Generation (Fuzz Testing)

ツールのバグを発見するための自動テストケース生成プロトコル。
エージェントがこのドキュメントを読めば、生成 → 検証 → バグ分類の全フローを実行できる。

## 概要

```
Step 1: サブエージェントがテストケースを N 件生成 (/tmp/pym_fuzz/NNN/)
Step 2: run_tests.sh で一括検証（ファイル中身は見ない）
Step 3: FAIL があればトリアージ（バグ or テスト作成ミス）
Step 4: 真のバグ → fixtures/ に移動 + known_bug.txt + KNOWN_BUGS.md に記録
```

## Step 1: テストケース生成

### 生成先

```
/tmp/pym_fuzz/
├── 001/
│   ├── input.py      # 先頭行: # kai: START-END
│   └── expected.py   # ツールを使わず手動で書いた期待出力
├── 002/
│   ├── input.py
│   └── expected.py
├── ...
└── run_tests.sh      # 検証スクリプト
```

ディレクトリ名は 3 桁ゼロ埋め連番（`001`, `002`, ...）。
既にディレクトリがある場合、最大番号の次から始める。

### サブエージェントへの委譲

テストケース生成はサブエージェント (Task tool, `subagent_type: "general-purpose"`) に委譲する。
メインエージェントのコンテキストを保護するため、**生成されたファイルの中身は読まない**。

#### サブエージェントプロンプト

以下のプロンプトをそのまま渡す。`{N}` は生成件数に置換する。
プロンプト内にコードブロックのネストがあるため、区切りとして `===PROMPT_START===` / `===PROMPT_END===` を使う。

===PROMPT_START===

あなたは Python コードリファクタリングツール "kai" のテストケース生成エージェントです。

## kai の動作

kai はPythonコード内の「構造的に同一のパターン」を見つけ、共通関数に抽出します。

## ルール

### フォーマット
- input.py の1行目は `# kai: START-END`（1-based行番号。マーカー行自体は数えない）
- START-END は抽出するブロックの行範囲（マーカーの次の行が1行目）
- expected.py にはマーカー行も含める（input.py と同じマーカー）

### 核心概念: 入力・差分・出力

ブロックを関数に抽出するには3つの情報が必要:

**入力 (Inputs)**: ブロック内で Load されるが、ブロック内で先に Store されていない変数。
つまり、外部から来る変数。ビルトイン (`print`, `len`, `range` 等) は除外。

**差分 (Divergences)**: 2つのブロックで構造的に同じ位置にあるが、値が異なるもの:
- Name divergence: 変数名が違う（例: `x` vs `y`）
- Literal divergence: リテラル値が違う（例: `1` vs `10`）
- 同じ値なら差分ではない（例: `x ** 2` と `y ** 2` → `2` は同値なので差分にならない）

**出力 (Outputs)**: ブロック内で Store され、ブロック後で使用される変数。
- 関数スコープ: ブロック後のコード（制御フローを遡りスコープ境界まで）で使われる store のみ
- モジュール/クラススコープ: 全 store を出力

### パラメータの決定

パラメータ = 入力変数 + リテラル差分。この順番で並ぶ:

1. **入力変数** — ブロック内で最初に Load される順（各変数は1回のみ）
   - divergent/non-divergent を問わず、全入力がパラメータになる
   - 各ブロックの呼び出しでは、そのブロック固有の変数名を引数に使う
2. **リテラル差分** — ブロック内の出現順
   - 各ブロックの呼び出しでは、そのブロック固有のリテラル値を引数に使う

Name divergence 自体はパラメータを追加しない。
名前が違っても同じ構造位置の入力は1つのパラメータにまとまる。

命名: `arg_0`, `arg_1`, `arg_2`, ...（入力とリテラル差分を通しで連番）

### 戻り値の決定

- 出力変数が入力でもある → パラメータ名 `arg_N` がそのまま戻り値にもなる
- 出力のみの変数 → `ret_0`, `ret_1`, ...（通しで連番）
- 戻り値が1つ: `return ret_0` + `ret_0 = func(...)`
- 戻り値が複数: `return ret_0, arg_0` + `ret_0, arg_0 = func(...)`
- 戻り値なし: return なし + `func(...)` のみ

### 関数本文

関数本文はブロック0（指定行範囲）のソーステキストをベースに:
- 入力変数 → 対応する `arg_N` に置換
- 差分リテラル → 対応する `arg_N` に置換
- 出力のみ変数 → 対応する `ret_N` に置換（入力兼出力は `arg_N` のまま）
- ローカル変数（入力でも出力でもない）→ ブロック0の名前をそのまま使う

### 関数配置
- モジュールスコープ → ファイル先頭に配置
- 関数スコープ → 関数 body の先頭（def 文の次の行）にネスト関数として配置
- クラススコープ → クラス定義の直前に配置（クラス外）

### 構造的マッチの条件
- AST 構造が同一（演算子、文の種類、属性名が一致）
- 変数名とリテラル値の違いは許容
- 文の数（window_size）が一致

### 制御フロー内のブロック
- if/for/while/with/try/match の内部にあるブロックも検出される
- ただし break/continue/return/yield がブロック直下にある場合は抽出不可（エラー）
  （ネストした for/while/def/lambda 内にあれば安全）

### self 属性
- `self.x = val` は属性への副作用であり、`self.x` 自体は戻り値にならない
- `self` はオブジェクト参照として入力パラメータになる

### Worked Examples

#### 例1: リテラル差分のみ（モジュールスコープ）

input.py:
```python
# kai: 1-3
a = 1
b = a + 2
print(b)
a = 10
b = a + 20
print(b)
```
分析:
- 入力: なし（a, b はブロック内で Store が先）
- 差分: Literal(1, 10), Literal(2, 20)
- 出力: a, b（モジュールスコープなので全 store。出力のみなので ret_0, ret_1）
- パラメータ: arg_0 (= 1/10), arg_1 (= 2/20)

expected.py:
```python
def extracted_func_0(arg_0, arg_1):
    ret_0 = arg_0
    ret_1 = ret_0 + arg_1
    print(ret_1)
    return ret_0, ret_1

# kai: 1-3
a, b = extracted_func_0(1, 2)
a, b = extracted_func_0(10, 20)
```

#### 例2: 入力 + リテラル差分（関数スコープ）

input.py:
```python
# kai: 4-5
def show(items):
    for x in items:
        a = x + 1
        print(a)
    for y in items:
        b = y + 10
        print(b)
```
分析:
- 入力: x/y（Load が先。divergent name → 各ブロックで異なる引数値）
- 差分: Name(x, y), Literal(1, 10)
- 出力: なし（ブロック後で a, b は使われない）
- パラメータ: arg_0 (= x/y, 入力), arg_1 (= 1/10, リテラル差分)

expected.py:
```python
# kai: 4-5
def show(items):
    def extracted_func_0(arg_0, arg_1):
        a = arg_0 + arg_1
        print(a)

    for x in items:
        extracted_func_0(x, 1)
    for y in items:
        extracted_func_0(y, 10)
```

#### 例3: 入力かつ出力（AugAssign）

input.py:
```python
# kai: 3-3
total = 0
total += 5
print(total)
total = 0
total += 10
print(total)
```
分析:
- 入力: total（AugAssign は Load + Store。Load が先）
- 差分: Literal(5, 10)
- 出力: total（ブロック後 `print(total)` で使用。入力でもあるので arg_0 のまま）
- パラメータ: arg_0 (= total), arg_1 (= 5/10)

expected.py:
```python
def extracted_func_0(arg_0, arg_1):
    arg_0 += arg_1
    return arg_0

# kai: 3-3
total = 0
total = extracted_func_0(total, 5)
print(total)
total = 0
total = extracted_func_0(total, 10)
print(total)
```

#### 例4: Non-divergent 入力 + self 属性

input.py:
```python
# kai: 4-5
class Rectangle:
    def __init__(self):
        self.x = 10
        self.y = self.x * 5
        self.x = 20
        self.y = self.x * 3
```
分析:
- 入力: self（non-divergent だが全入力がパラメータになる）
- 差分: Literal(10, 20), Literal(5, 3)
- 出力: なし（self.x, self.y は属性副作用。self 自体は Store されない）
- パラメータ: arg_0 (= self), arg_1 (= 10/20), arg_2 (= 5/3)

expected.py:
```python
# kai: 4-5
class Rectangle:
    def __init__(self):
        def extracted_func_0(arg_0, arg_1, arg_2):
            arg_0.x = arg_1
            arg_0.y = arg_0.x * arg_2

        extracted_func_0(self, 10, 5)
        extracted_func_0(self, 20, 3)
```

## タスク

/tmp/pym_fuzz/ 以下に {N} 件のテストケースを生成してください。
ディレクトリ名は3桁連番（既存の最大番号+1 から開始）。

以下のカテゴリを多様にカバーすること:

1. 基本パターン: 単純代入、算術演算、関数呼び出し
2. 関数スコープ: def 内のブロック（ネスト関数として抽出）
3. クラススコープ: class 内のブロック（クラス外に抽出）
4. 制御フロー内: if/for/while 内のブロック
5. 出力あり: ブロック後で変数が使われるケース
6. 入力+出力: 同じ変数がパラメータかつ戻り値
7. リテラル差分: 数値、文字列、ブール値の差分
8. 同値リテラル: 同じリテラルがパラメータ化されないことの確認
9. 複数ブロック: 3つ以上の同一パターン
10. 複合式: f-string, リスト内包表記, lambda, タプルアンパック
11. AugAssign: +=, -= 等
12. self 属性: self.x への代入（self 自体は入力、self.x 代入は戻り値にならない）
13. エッジケース: 空行を含む、深いネスト、長い式

各テストケースは 5-20 行程度。
複雑すぎるケースは手動検証が困難なので避ける。
テストケースの正しさに自信がない場合は、そのディレクトリに `uncertain.txt` を作成する。

重要: expected.py はツールを使わず、上記ルールに従って自分で書くこと。

===PROMPT_END===

#### サブエージェント起動例

```
Task tool:
  subagent_type: "general-purpose"
  description: "Generate N fuzz test cases"
  prompt: (上記 ===PROMPT_START=== から ===PROMPT_END=== までを渡す。{N} を具体的な数字に置換)
```

生成件数の目安: 1回あたり 20-30 件。
サブエージェントが終了したら、生成されたファイルの中身は見ず、Step 2 に進む。

## Step 2: 一括検証

### 検証スクリプト

`/tmp/pym_fuzz/run_tests.sh` を使う。なければ以下で作成:

```bash
#!/bin/bash
TOOL="$HOME/repos/pym/target/release/kai"
FUZZ_DIR="/tmp/pym_fuzz"
PASS=0; FAIL=0; ERROR=0; SKIP=0; FAIL_LIST=""

# Build release binary first
cargo build --release --manifest-path="$HOME/repos/pym/Cargo.toml" 2>/dev/null

for dir in "$FUZZ_DIR"/[0-9]*/; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    input="$dir/input.py"
    expected="$dir/expected.py"
    [ -f "$input" ] || continue
    [ -f "$expected" ] || continue

    # Skip uncertain cases
    if [ -f "$dir/uncertain.txt" ]; then
        echo "  SKIP:   $name (uncertain)"
        SKIP=$((SKIP + 1))
        continue
    fi

    marker=$(head -1 "$input")
    range=$(echo "$marker" | sed 's/# kai: //')
    start=$(echo "$range" | cut -d- -f1)
    end=$(echo "$range" | cut -d- -f2)

    actual=$("$TOOL" "$input" "$start" "$end" --no-interactive 2>/dev/null)
    exit_code=$?

    if [ $exit_code -ne 0 ]; then
        echo "  ERROR:  $name (exit code $exit_code)"
        ERROR=$((ERROR + 1))
        FAIL_LIST="$FAIL_LIST $name"
        continue
    fi

    expected_content=$(cat "$expected")
    if [ "$actual" = "$expected_content" ]; then
        echo "  PASS:   $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL:   $name"
        echo "$actual" > "$dir/actual.py"
        FAIL=$((FAIL + 1))
        FAIL_LIST="$FAIL_LIST $name"
    fi
done

echo ""
echo "--- $PASS passed, $FAIL failed, $ERROR errors, $SKIP skipped ---"
[ -n "$FAIL_LIST" ] && echo "Failed/Error:$FAIL_LIST"
```

### 実行方法

```bash
# release ビルド（初回のみ）
cargo build --release

# テスト実行
bash /tmp/pym_fuzz/run_tests.sh
```

出力から FAIL/ERROR のケース番号だけを記録する。
**PASS したケースの中身は見ない**（コンテキスト節約）。

## Step 3: トリアージ

FAIL したケースについて、1件ずつ以下を確認する:

### 3a. ツール出力を確認

```bash
# actual.py が自動生成されている
cat /tmp/pym_fuzz/NNN/actual.py
```

### 3b. expected.py と比較

```bash
diff /tmp/pym_fuzz/NNN/expected.py /tmp/pym_fuzz/NNN/actual.py
```

### 3c. 判定

| actual.py の状態 | 判定 | アクション |
|---|---|---|
| ツール出力が正しい、expected.py が間違い | テスト作成ミス | expected.py を修正 |
| ツール出力が間違い、expected.py が正しい | **真のバグ** | Step 4 へ |
| 両方怪しい | 要調査 | input.py のルール違反を確認 |

判定基準:
- 出力された Python コードが構文的に正しいか（`python -c "import ast; ast.parse(open('actual.py').read())"` で検証）
- 抽出された関数のロジックが元コードと等価か
- パラメータの順序・名前が上記ルール通りか

## Step 4: バグ登録

### 4a. フィクスチャに移動

```bash
# ケース名を決める（バグの内容を反映した snake_case）
FIXTURE_NAME="descriptive_bug_name"
cp -r /tmp/pym_fuzz/NNN/ tests/fixtures/$FIXTURE_NAME/

# actual.py と uncertain.txt は不要
rm -f tests/fixtures/$FIXTURE_NAME/actual.py
rm -f tests/fixtures/$FIXTURE_NAME/uncertain.txt
```

### 4b. known_bug.txt を作成

テストが FAIL しても panic しないように `known_bug.txt` を配置:

```
tests/fixtures/$FIXTURE_NAME/known_bug.txt
```

内容は1行でバグの説明:

```
Same literal at non-divergent position is incorrectly parameterized
```

### 4c. KNOWN_BUGS.md に記録

```markdown
## Bug N: 簡潔なタイトル

- **フィクスチャ**: `tests/fixtures/FIXTURE_NAME/`
- **発見元**: fuzz test NNN
- **症状**: ツールが何を間違えるか
- **期待動作**: 正しくはどうなるべきか
- **原因仮説**: (わかれば)
- **関連ファイル**: `src/xxx.rs`
```

### 4d. テストで確認

```bash
cargo test
```

新しいフィクスチャが `KNOWN_BUG` として認識されることを確認。

## コンテキスト保護のガイドライン

| やること | やらないこと |
|---------|------------|
| run_tests.sh の出力（PASS/FAIL 行）を見る | 個々のテストケースの input.py/expected.py を見る |
| FAIL したケースだけ actual.py と diff を見る | PASS した全ケースの中身を確認する |
| サブエージェントに生成を委譲する | メインエージェントで N 件のファイルを生成する |
| バグ判定後に必要なケースだけ fixtures/ に移動 | 全ケースを fixtures/ にコピーする |

## 既存カバレッジ（参考）

現在のフィクスチャでカバー済みのパターン:

| カテゴリ | フィクスチャ例 |
|---------|-------------|
| 基本代入 | `simple_assignment`, `literal_divergence` |
| 関数スコープ | `inside_function`, `nested_function`, `deep_nesting` |
| クラススコープ | `class_method`, `method_in_class`, `class_with_after_code` |
| 制御フロー | `for_body_scan`, `if_body_scan`, `while_body_scan`, `try_body_scan`, `match_body_scan` |
| 出力あり | `function_with_output`, `with_returns`, `multiple_returns` |
| リテラル差分 | `function_literal_divergence`, `mixed_divergence` |
| 同値リテラル | `power_same_exponent` |
| 3ブロック | `three_blocks`, `three_blocks_literal` |
| f-string | `fstring_divergence`, `fstring_concat_divergence`, `fstring_escape_segment` |
| 内包表記 | `comprehension_divergence` |
| lambda | `lambda_divergence` |
| AugAssign | `aug_assign` |
| self 属性 | `self_attr`, `self_attr_rename_bug` |
| タプルアンパック | `tuple_unpacking` |
| エラーケース | `error_return_not_extractable`, `error_yield_not_extractable`, `error_single_match` |
| マルチファイル | `multi_simple`, `multi_three_files`, `multi_with_returns` |
| after_block | `after_block_scope_boundary` |

生成時はこれらと重複しない、未カバーの組み合わせを狙うこと。
