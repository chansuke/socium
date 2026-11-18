class ParseError(Exception):
    def __init__(self, message, line):
        super().__init__(f"line {line}: {message}")
        self.line = line


def parse(text):
    """See SPEC.md."""
    raise NotImplementedError
