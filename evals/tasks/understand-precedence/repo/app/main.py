import tomllib

from app import config


def read_file(path="app.toml"):
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except FileNotFoundError:
        return {}


def start():
    file_values = read_file()
    values = config.load(file_values)
    values = config.finalize(values, file_values)
    return values
