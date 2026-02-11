# pym — Python Extract Method

A deterministic, AST-based refactoring tool that finds structurally identical code blocks in a Python file and extracts them into a shared function.

## How It Works

Given a reference code block (by line range), `pym`:

1. **Normalizes** the AST — variable names and literals are abstracted away, leaving only the structural "shape" of the code.
2. **Scans** the entire file for blocks with an identical normalized hash (sliding window over top-level statements).
3. **Analyzes** variable scope — determines which variables become function parameters (inputs) and return values (outputs).
4. **Extracts divergences** — variables and literals that differ between matching blocks become parameters.
5. **Rewrites** the source — generates a new function definition and replaces each matching block with a call to it.

No LLM or external API is used. The transformation is fully static and deterministic.

## Installation

Requires Rust 1.91+ (edition 2024).

```sh
git clone https://github.com/<user>/pym.git
cd pym
cargo build --release
# binary at target/release/pym
```

## Usage

```
pym <FILE> <START_LINE> <END_LINE> [OPTIONS]
```

### Basic Example

Given `example.py`:

```python
a = 1
b = a + 2
c = 3
x = 100
y = x + 200
c = 3
```

Run:

```sh
pym example.py 2 3
```

Output (refactored source):

```python
def extracted_func_0(arg_0, arg_1):
    a = arg_0
    b = a + arg_1

extracted_func_0(1, 2)
c = 3
extracted_func_0(100, 200)
c = 3
```

Lines 2–3 (`a = 1; b = a + 2`) and lines 5–6 (`x = 100; y = x + 200`) are structurally identical. The differing literals (`1`/`100`, `2`/`200`) become parameters.

### Options

| Flag | Description |
|------|-------------|
| `--diff` | Show a unified diff instead of the full refactored source |
| `--write` | Write the result back to the file in-place |
| `--name <NAME>` | Custom function name (default: `extracted_func_0`) |
| `--args <ARGS>` | Custom parameter names, comma-separated (e.g. `"a, b"`) |
| `--select <SEL>` | Replace only specific matched blocks by 1-based index (e.g. `"1,3"`) |

### More Examples

**Custom function and parameter names:**

```sh
pym example.py 2 3 --name compute --args "x, y"
```

```python
def compute(x, y):
    a = x
    b = a + y

compute(1, 2)
c = 3
compute(100, 200)
c = 3
```

**Unified diff output:**

```sh
pym example.py 2 3 --diff
```

**Write back to file:**

```sh
pym example.py 2 3 --write
```

**Select which blocks to replace** (skip block 2):

```sh
pym example.py 2 3 --select 1
```

## Architecture

| Module | Purpose |
|--------|---------|
| `normalize.rs` | AST normalization visitor, structural hashing |
| `scan.rs` | Sliding-window block scanner |
| `scope.rs` | Variable scope analysis (inputs/outputs) |
| `diff_extract.rs` | Cross-block divergence extraction (names, literals) |
| `rewrite.rs` | Code generation, replacement, unified diff |
| `lib.rs` | Public API and pipeline orchestration |
| `main.rs` | CLI interface |

## Limitations

- Top-level statements only (no scanning inside class/function bodies)
- Single-file refactoring
- All external `Load` variables become parameters (including builtins like `print`)

## License

MIT
