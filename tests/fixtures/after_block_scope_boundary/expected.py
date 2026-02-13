# kai: 5-6
def process(items):
    def extracted_func_0(arg_0, arg_1):
        arg_0 = arg_0 + arg_1
        print(arg_0)
        return arg_0

    total = 0
    for x in items:
        total = extracted_func_0(total, x)
    for y in items:
        total = extracted_func_0(total, y)
    return total
