# pym

Python Extract Method refactoring tool. Finds structurally identical code blocks and extracts them into a shared function.

## Example

```python
# example.py
a = 1
b = a + 2
c = 3
x = 100
y = x + 200
```

```sh
pym example.py 1 2
```

```python
def extracted_func_0(arg_0, arg_1):
    a = arg_0
    b = a + arg_1

extracted_func_0(1, 2)
c = 3
extracted_func_0(100, 200)
```

Lines 1-2 and 4-5 are structurally identical. The differing literals become parameters.

## Usage

```
pym FILE [FILE...] START END [OPTIONS]
```

```sh
pym example.py 1 2                    # interactive mode (default)
pym example.py 1 2 --diff             # unified diff output
pym example.py 1 2 --write            # write back to file
pym a.py b.py c.py 1 2               # multi-file refactoring
pym example.py 1 2 --no-interactive   # non-interactive (for scripts/CI)
```

## Installation

```sh
curl -LsSf https://github.com/morim3/pym/releases/latest/download/pym-installer.sh | sh
```

Or build from source (requires Rust 1.91+):

```sh
cargo install --git https://github.com/morim3/pym
```

## License

MIT
