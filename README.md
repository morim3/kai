# pym — Python Extract Method

A deterministic, AST-based refactoring tool that finds structurally identical code blocks across Python files and extracts them into a shared function.

## How It Works

Given a reference code block (by line range), `pym`:

1. **Normalizes** the AST — variable names and literals are abstracted away, leaving only the structural "shape" of the code.
2. **Scans** the file (and optionally other files) for blocks with an identical normalized hash — including inside functions, classes, and nested scopes.
3. **Analyzes** variable scope — determines which variables become function parameters (inputs) and return values (outputs).
4. **Extracts divergences** — variables and literals that differ between matching blocks become parameters.
5. **Rewrites** the source — generates a new function definition and replaces each matching block with a call to it.

No LLM or external API is used. The transformation is fully static and deterministic.

## Installation

Requires Rust 1.91+ (edition 2024).

```sh
git clone https://github.com/morim3/pym.git
cd pym
cargo build --release
# binary at target/release/pym
```

## Usage

```
pym FILE [FILE...] START END [OPTIONS]
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

Lines 2-3 (`a = 1; b = a + 2`) and lines 5-6 (`x = 100; y = x + 200`) are structurally identical. The differing literals (`1`/`100`, `2`/`200`) become parameters.

### Options

| Flag | Description |
|------|-------------|
| `--diff` | Show a unified diff instead of the full refactored source |
| `--write` | Write the result back to the file(s) in-place |
| `--name <NAME>` | Custom function name (default: `extracted_func_0`) |
| `-i`, `--interactive` | Interactive mode: review and customize each step |

### Multi-File Refactoring

`pym` can scan multiple files for structurally matching blocks:

```sh
pym main.py utils.py helpers.py 2 3
```

The extracted function is placed in the first file (target), and other files get a `from <module> import <func>` statement added.

### Interactive Mode

Use `-i` for step-by-step control over the extraction:

```sh
pym example.py 2 3 -i
```

Interactive mode lets you:
- Select which matched blocks to include
- Choose a custom function name
- Rename parameters and return values
- Add extra return values
- Preview the result before writing

### More Examples

**Custom function name:**

```sh
pym example.py 2 3 --name compute
```

**Unified diff output:**

```sh
pym example.py 2 3 --diff
```

**Write back to file:**

```sh
pym example.py 2 3 --write
```

## Architecture

| Module | Purpose |
|--------|---------|
| `normalize.rs` | AST normalization visitor, structural hashing |
| `scan.rs` | Sliding-window block scanner, scope traversal |
| `scope.rs` | Variable scope analysis (inputs/outputs), signature unification |
| `diff_extract.rs` | Cross-block divergence extraction (names, literals) |
| `rewrite.rs` | Code generation, replacement, unified diff |
| `interactive.rs` | Interactive mode (dialoguer-based step-by-step flow) |
| `lib.rs` | Public API and pipeline orchestration |
| `main.rs` | CLI interface |

## License

MIT
