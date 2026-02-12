def extracted_func_0(arg_0):
    ret_0 = arg_0
    ret_1 = ret_0 * 2
    ret_2 = ret_0 + ret_1
    return ret_0, ret_1, ret_2

# kai: 2-4
x, y, z = extracted_func_0(10)
print(y, z)
a, b, c = extracted_func_0(20)
print(b, c)
