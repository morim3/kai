# kai: 4-5
class Foo:
    def setup(self):
        def extracted_func_0(arg_0, arg_1, arg_2):
            arg_0.x = arg_1
            arg_0.y = arg_0.x + arg_2

        extracted_func_0(self, 1, 2)
        extracted_func_0(self, 10, 20)
