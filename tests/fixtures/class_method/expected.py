# kai: 3-4
def extracted_func_0(arg_0, arg_1):
    ret_0 = arg_0
    ret_1 = ret_0 + arg_1
    return ret_0, ret_1

class Config:
    x, y = extracted_func_0(1, 2)
    a, b = extracted_func_0(10, 20)
