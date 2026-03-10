# kai: 2-3
def f(x):
    def extracted_func_0(arg_0):
        ret_0 = f"{arg_0!r}"
        return ret_0

    msg = extracted_func_0(x)
    print(msg)

def g(y):
    msg = f"{y!s}"
    print(msg)
