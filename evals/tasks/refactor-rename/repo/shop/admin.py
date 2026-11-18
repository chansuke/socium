from shop import users


def describe(user_id):
    return f"#{user_id}: {users.fetch_usr(user_id) or 'unknown'}"
