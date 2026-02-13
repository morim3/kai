# kai: 4-5
def outer_a():
    def inner():
        x = 1
        print(x)
    inner()

def outer_b():
    def helper():
        y = 10
        print(y)
    helper()
