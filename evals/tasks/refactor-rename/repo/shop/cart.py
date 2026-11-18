from shop.users import fetch_usr


def owner_label(cart):
    name = fetch_usr(cart["user_id"])
    return name.title() if name else "guest"
