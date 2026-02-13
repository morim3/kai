# Fuzz Testing Session Log

## Session 1 (2026-02-13)

### 結果
- 生成: 087-111 (25件)
- 結果: 85/85 PASS (既存60件 + 新規25件)
- バグ発見: 0件

### カバレッジ
基本パターン、関数/クラス/モジュールスコープ、制御フロー(while/for/if)、
入力+出力、AugAssign(-=, *=)、self属性、同値リテラル、4ブロック、
lambda、リスト内包表記、深いネスト、複数出力、空行、
文字列/ブール/数値リテラル差分

### 迷った点・気づき
1. **サブエージェントが保守的すぎる可能性**: 25件全 PASS はバグ発見率としては低い。
   サブエージェントがルールを正確に理解して expected.py を書いているため、
   ツールの「正しい」動作と一致するケースしか生成されない。
   バグ発見のためには、ルールの境界条件やあいまいな部分を攻めるケースが必要。

2. **未カバーのエッジケース候補**（次回セッションで試すべき）:
   - 同じ変数が異なるスコープで input/output になるケース
   - for-else, while-else 内のブロック
   - try-except-finally 内のブロック
   - 複数の制御フロー構造にまたがるブロック
   - f-string 内のリテラルセグメント差分
   - 入力変数が3つ以上あるケース
   - 同名変数が Load と Store の両方で複雑に絡むケース
   - タプルアンパック代入がブロック内にあるケース
   - `a, b = func()` のような Store がタプルアンパック形式のケース
   - ネストした内包表記（`[x for x in [y for y in ...]]`）

3. **プロンプト改善案**:
   - 「ルール通りに書いたケース」だけでなく「ルールの解釈が曖昧なケース」も
     意図的に生成するよう指示すると、バグ発見率が上がるかもしれない
   - uncertain.txt を積極的に使って「自信がないケース」を増やすべき

## Session 2 (2026-02-13)

### アプローチ
Approach C (two-stage): サブエージェント1が input.py のみ生成（expected.py なし）→ ツール実行 → property check（出力が valid Python か？元コードと等価か？）

### 結果
- 生成: 112-136 (25件の input.py)
- Property check 1 (valid Python?):
  - VALID: 20件 (112-115, 117-130, 134, 136)
  - EMPTY_OUTPUT: 5件 (116, 131-133, 135) — 正当な拒否 or スコープ外
- Property check 2 (元コードと実行結果が等価?):
  - EQUIV: 20/20件 (128は両方FileNotFoundErrorだが、ファイルがあれば等価)
- バグ発見: 0件 (136 は bash 変数が escape を解釈する false alarm)
- expected.py 作成: 20件 (verified actual.py をコピー)
- 最終結果: 105/105 PASS

### EMPTY_OUTPUT の分析
| Case | 原因 | カテゴリ |
|------|------|---------|
| 116 | `return counter` in block → SafetyChecker rejects | 正当な拒否 |
| 131 | 別クラスの classmethod 間 — スコープ外マッチ | テスト設計の問題 |
| 132 | 別クラスの `super().__init__()` — 別スコープ | テスト設計の問題 |
| 133 | 別クラスの @property — 別スコープ | テスト設計の問題 |
| 135 | 別クラスのメソッド — 別スコープ | テスト設計の問題 |

### 迷った点・気づき
1. **bash 変数の escape 解釈**: `echo "$output"` は `r"\n"` を actual newline に変換する。
   property check スクリプトで `echo "$actual"` 経由で actual.py を書くと false positive になる。
   → `cargo run ... > file` で直接リダイレクトすべき。

2. **Approach C の評価**: input.py 生成の多様性は向上（別クラス間のマッチ試行、raw string、
   context manager 等）。ただし検証は execution-based property check が最も信頼性高い。
   sub-agent にルールベースで expected.py を書かせるよりも、ツール出力を実行して検証する方が
   false positive/negative が少ない。

3. **scanner のスコープ制限**: 別々の class 定義内のメソッドは sibling scope として
   スキャンされない（131-133, 135）。これは設計上の制限であり、バグではない。
