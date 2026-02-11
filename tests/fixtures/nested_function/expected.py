# pym: 4-5
def outer():
    def inner():
        def extracted_func_0(arg_0, arg_1):
            a = arg_0
            b = a + arg_1

        extracted_func_0(1, 2)
        extracted_func_0(10, 20)
