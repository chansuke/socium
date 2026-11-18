import os

from app import defaults

# Keys an operator pins in the file; nothing else may change them.
LOCKED = {"port"}


def load(file_values):
    """Defaults, then the file, then the environment."""
    values = dict(defaults.VALUES)
    values.update(file_values)
    port = os.environ.get("APP_PORT")
    if port:
        values["port"] = int(port)
    return values


def finalize(values, file_values):
    for key in LOCKED & file_values.keys():
        values[key] = file_values[key]
    return values
