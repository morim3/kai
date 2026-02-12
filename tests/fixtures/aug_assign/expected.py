def extracted_func_0(arg_0, arg_1):
    ret_0 = arg_1
    arg_0 += ret_0
    return ret_0, arg_0

# kai: 3-4
total = 0
count, total = extracted_func_0(total, 5)
total *= 2
print(total)
total = 0
amount, total = extracted_func_0(total, 10)
total *= 2
print(total)
