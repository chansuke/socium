import argparse
import json

STORE = "tasks.json"


def load():
    with open(STORE) as f:
        return json.load(f)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="todo")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list")
    args = parser.parse_args(argv)
    if args.command == "list":
        for task in load():
            mark = "x" if task["done"] else " "
            print(f"- [{mark}] {task['title']}")


if __name__ == "__main__":
    main()
