from shop import users


def describe(user_id):
    return f"#{user_id}: {users.fetch_user(user_id) or 'unknown'}"
