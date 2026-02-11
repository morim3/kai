# pym: 5-6
def outer():
    class Processor:
        def run(self):
            def extracted_func_0(arg_0, arg_1):
                a = arg_0
                b = a + arg_1

            extracted_func_0(1, 2)
            extracted_func_0(10, 20)
