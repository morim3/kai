# Progress: Session on commit 9cef867

## Operational Mode
FEATURE_MODE

## Session Summary

### 完了した作業
1. **パイプライン関数 `extract_method()` を `lib.rs` に抽出**
   - `main.rs` のコアロジックを `pym::extract_method(source, start_line, end_line)` として公開
   - `main.rs` は CLI パース + `extract_method()` 呼び出しだけに簡素化
2. **統合テストフレームワーク構築** (`tests/integration.rs`)
   - `tests/fixtures/*/` を自動走査するデータ駆動テスト
   - `input.py` + `# pym: START-END` マーカー + `expected.py` の規約
   - `expected_error.txt` によるエラーケースのテスト
   - `known_bug.txt` による既知バグの可視化（テストスイートは通過、修正時に自動検出）
3. **フィクスチャ作成** (6件)
   - `simple_assignment` — 基本2ブロック抽出 ✅
   - `three_blocks` — 3ブロック同時抽出 ✅
   - `for_loop` — 制御構造含むブロック ✅
   - `error_single_match` — マッチ不足エラー ✅
   - `with_returns` — 戻り値ありケース 🐛 known_bug
   - `literal_divergence` — リテラル差異ケース 🐛 known_bug
4. **`rewrite.rs` の `end_to_end_refactoring` を削除** — 統合テストに移行済み
5. **design_doc.md に Phase 5 追加** — CLI使いやすさ向上

### 発見したバグ (known_bug フィクスチャとして記録済み)
- **`with_returns`**: 関数本体に `return ret_0` を生成するが `ret_0` が未定義。戻り値の変数マッピングが壊れている。
- **`literal_divergence`**: 2番目のブロック置換で代入先変数名が欠落 (` = extracted_func_0(...)`)。`x` が output (ret_0) と body 内の名前の両方で使われ、置換が破綻。

## Next Actions

### 1. デフォルト出力形式の変更 (Phase 5 の一部)
- `--write` なしの出力を Unified Diff → リファクタリング後コード全文に変更
- 旧 diff 出力は `--diff` オプションに移行
- **理由**: 統合テストで CLI 出力と `expected.py` を直接比較できるようになる。バグ修正の検証もやりやすくなるため、バグ修正より先にやるべき。

### 2. バグ修正
- `with_returns`: `scope.rs` / `rewrite.rs` の戻り値マッピングロジックを修正
- `literal_divergence`: `rewrite.rs` の `replace_identifier` で変数名が消える問題を修正
- 修正後、`known_bug.txt` を削除すればテストが自動的に正規テストになる

### 3. より難しいフィクスチャの追加
- 関数内の関数 (nested function)
- クラスメソッド内のブロック
- インデントされたブロック (if/for 内の類似コード)
- 複数の戻り値を持つケース
- 変数名の衝突があるケース

### 4. Phase 5 残り機能
- `--select` ブロック選択機能
- `--name` / `--args` 命名カスタマイズ機能
