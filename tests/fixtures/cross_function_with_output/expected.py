def extracted_func_0(arg_0, arg_1):
    a = arg_0
    ret_0 = a + arg_1
    return ret_0

# pym: 3-4
def foo():
    b = extracted_func_0(1, 2)
    print(b)

def bar():
    y = extracted_func_0(10, 20)
    print(y)
