# kai: 5-8
def process_orders(orders):
    for order in orders:
        if order.status == "pending":
            total = order.price
            tax = total * 0.1
            final = total + tax
            print(f"Pending: {final}")

    for order in orders:
        if order.status == "shipped":
            total = order.price
            tax = total * 0.2
            final = total + tax
            print(f"Shipped: {final}")
