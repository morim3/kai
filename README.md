# kai

Python Extract Method refactoring tool. Finds structurally identical code blocks and extracts them into a shared function.

## Example

```python
# analytics.py
total_price = 0
for item in cart:
    total_price += item["price"]
avg_price = total_price / len(cart)

total_weight = 0
for item in cart:
    total_weight += item["weight"]
avg_weight = total_weight / len(cart)

print(avg_price, avg_weight)

def summarize(entries):
    total_score = 0
    for item in entries:
        total_score += item["score"]
    avg_score = total_score / len(entries)
    return avg_score

class Dashboard:
    def refresh(self, records):
        total_clicks = 0
        for item in records:
            total_clicks += item["clicks"]
        avg_clicks = total_clicks / len(records)
        self.display(avg_clicks)
```

```sh
# command line
kai analytics.py 1 4
```

```python
# output
def avg_by_key(items, key):
    total = 0
    for item in items:
        total += item[key]
    avg = total / len(items)
    return avg

avg_price = avg_by_key(cart, "price")
avg_weight = avg_by_key(cart, "weight")
print(avg_price, avg_weight)

def summarize(entries):
    avg_score = avg_by_key(entries, "score")
    return avg_score

class Dashboard:
    def refresh(self, records):
        avg_clicks = avg_by_key(records, "clicks")
        self.display(avg_clicks)
```

4 blocks across module level, a function, and a class method — all detected and extracted into one shared function. In interactive mode (default), you choose the function name and parameter names.

## Usage

```
kai FILE [FILE...] START END [OPTIONS]
```

```sh
kai example.py 1 2                    # interactive mode (default)
kai example.py 1 2 --diff             # unified diff output
kai example.py 1 2 --write            # write back to file
kai a.py b.py c.py 1 2               # multi-file refactoring
kai example.py 1 2 --no-interactive   # non-interactive (for scripts/CI)
```

## Installation

```sh
curl -LsSf https://github.com/morim3/kai/releases/latest/download/kai-installer.sh | sh
```

Or build from source (requires Rust 1.91+):

```sh
cargo install --git https://github.com/morim3/kai
```

## License

MIT
