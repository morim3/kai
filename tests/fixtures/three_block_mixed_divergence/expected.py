def extracted_func_0(arg_0, arg_1):
    ret_0 = arg_0
    ret_1 = arg_1
    return ret_0, ret_1

# kai: 2-4
def f():
    x, y = extracted_func_0("alpha", 100)
    print(x, y)

def g():
    x, y = extracted_func_0("beta", 100)
    print(x, y)

def h():
    x, y = extracted_func_0("gamma", 999)
    print(x, y)
