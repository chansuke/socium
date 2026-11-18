from shop.users import fetch_user


def owner_label(cart):
    name = fetch_user(cart["user_id"])
    return name.title() if name else "guest"
