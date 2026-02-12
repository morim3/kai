def extracted_func_0(arg_0, arg_1):
    ret_0 = lambda x: x + arg_0
    print(ret_0(arg_1))
    return ret_0

# kai: 2-3
fn_a = extracted_func_0(1, 10)
fn_b = extracted_func_0(2, 20)
