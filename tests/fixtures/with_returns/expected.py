def extracted_func_0(arg_0, arg_1):
    ret_0 = arg_1
    arg_0 = arg_0 + ret_0
    return ret_0, arg_0

# kai: 3-4
total = 0
x, total = extracted_func_0(total, 10)
print(total)
y, total = extracted_func_0(total, 20)
print(total)
