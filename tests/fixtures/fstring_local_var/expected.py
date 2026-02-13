# kai: 5-8
def process_orders(orders):
    def extracted_func_0(arg_0, arg_1, arg_2):
        total = arg_0.price
        tax = total * arg_1
        final = total + tax
        print(f"{arg_2}{final}")

    for order in orders:
        if order.status == "pending":
            extracted_func_0(order, 0.1, "Pending: ")

    for order in orders:
        if order.status == "shipped":
            extracted_func_0(order, 0.2, "Shipped: ")
