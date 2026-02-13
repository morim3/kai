def extracted_func_0(arg_0, arg_1, arg_2, arg_3, arg_4):
    ret_0 = -arg_0
    ret_1 = arg_1 + arg_2
    ret_2 = arg_3 + arg_4.decode()
    return ret_0, ret_1, ret_2

# kai: 2-4
x, y, msg = extracted_func_0(1, 2, 3j, r"\n raw", b"bytes")
print(x, y, msg)
a, b, txt = extracted_func_0(99, 20, 30j, r"\t raw", b"hello")
print(a, b, txt)
