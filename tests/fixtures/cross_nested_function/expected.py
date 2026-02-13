def extracted_func_0(arg_0):
    x = arg_0
    print(x)

# kai: 4-5
def outer_a():
    def inner():
        extracted_func_0(1)
    inner()

def outer_b():
    def helper():
        extracted_func_0(10)
    helper()
