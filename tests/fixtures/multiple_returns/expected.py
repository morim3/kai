def extracted_func_0(arg_0):
    x = arg_0
    ret_0 = x * 2
    ret_1 = x + ret_0
    return ret_0, ret_1

# kai: 2-4
y, z = extracted_func_0(10)
print(y, z)
b, c = extracted_func_0(20)
print(b, c)
