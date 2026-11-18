import argparse
import json

STORE = "tasks.json"


def load():
    with open(STORE) as f:
        return json.load(f)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="todo")
    commands = parser.add_subparsers(dest="command", required=True)
    listing = commands.add_parser("list")
    listing.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if args.command == "list" and args.json:
        print(json.dumps([{"title": t["title"], "done": t["done"]} for t in load()]))
    elif args.command == "list":
        for task in load():
            mark = "x" if task["done"] else " "
            print(f"- [{mark}] {task['title']}")


if __name__ == "__main__":
    main()
