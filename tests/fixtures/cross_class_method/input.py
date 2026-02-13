# kai: 5-6
class Animal:
    @classmethod
    def create(cls):
        name = "dog"
        obj = cls(name)
        print(obj)

class Vehicle:
    @classmethod
    def make(cls):
        label = "car"
        obj = cls(label)
        print(obj)
