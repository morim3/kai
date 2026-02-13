# kai: 5-6
def process(items):
    total = 0
    for x in items:
        total = total + x
        print(total)
    for y in items:
        total = total + y
        print(total)
    return total
