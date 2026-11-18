USERS = {1: "ada", 2: "grace"}


def fetch_usr(user_id):
    """The user's name, or None."""
    return USERS.get(user_id)
