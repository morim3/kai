def extracted_func_0(arg_0, arg_1):
    name = arg_1
    ret_0 = arg_0(name)
    return ret_0

# kai: 5-6
class Animal:
    @classmethod
    def create(cls):
        obj = extracted_func_0(cls, "dog")
        print(obj)

class Vehicle:
    @classmethod
    def make(cls):
        obj = extracted_func_0(cls, "car")
        print(obj)
