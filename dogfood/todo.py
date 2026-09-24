#!/usr/bin/env python3
"""A small todo CLI: add, list, done, delete. Persists to todos.json."""

import argparse
import json
import sys
from pathlib import Path

DEFAULT_FILE = Path("todos.json")


def load(path: Path) -> dict:
    """Load the todo store: {"next_id": int, "todos": [{id, text, done}, ...]}."""
    if path.exists():
        try:
            data = json.loads(path.read_text())
        except (json.JSONDecodeError, OSError):
            return {"next_id": 1, "todos": []}
        data.setdefault("next_id", 1)
        data.setdefault("todos", [])
        return data
    return {"next_id": 1, "todos": []}


def save(path: Path, data: dict) -> None:
    path.write_text(json.dumps(data, indent=2) + "\n")


def cmd_add(args) -> None:
    data = load(args.file)
    todo = {"id": data["next_id"], "text": args.text, "done": False}
    data["todos"].append(todo)
    data["next_id"] += 1
    save(args.file, data)
    print(f"Added todo {todo['id']}: {todo['text']}")


def cmd_list(args) -> None:
    data = load(args.file)
    if not data["todos"]:
        print("No todos.")
        return
    for todo in data["todos"]:
        mark = "x" if todo["done"] else " "
        print(f"[{mark}] {todo['id']}: {todo['text']}")


def cmd_done(args) -> None:
    data = load(args.file)
    for todo in data["todos"]:
        if todo["id"] == args.id:
            todo["done"] = True
            save(args.file, data)
            print(f"Marked todo {todo['id']} done: {todo['text']}")
            return
    print(f"Error: no todo with id {args.id}", file=sys.stderr)
    sys.exit(1)


def cmd_delete(args) -> None:
    data = load(args.file)
    for i, todo in enumerate(data["todos"]):
        if todo["id"] == args.id:
            removed = data["todos"].pop(i)
            save(args.file, data)
            print(f"Deleted todo {removed['id']}: {removed['text']}")
            return
    print(f"Error: no todo with id {args.id}", file=sys.stderr)
    sys.exit(1)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="A small todo CLI")
    parser.add_argument("--file", type=Path, default=DEFAULT_FILE,
                        help="path to the todos file (default: todos.json)")
    sub = parser.add_subparsers(dest="command", required=True)

    p_add = sub.add_parser("add", help="add a todo")
    p_add.add_argument("text", help="todo text")
    p_add.set_defaults(func=cmd_add)

    p_list = sub.add_parser("list", help="list todos")
    p_list.set_defaults(func=cmd_list)

    p_done = sub.add_parser("done", help="mark a todo done")
    p_done.add_argument("id", type=int, help="todo id")
    p_done.set_defaults(func=cmd_done)

    p_delete = sub.add_parser("delete", help="delete a todo")
    p_delete.add_argument("id", type=int, help="todo id")
    p_delete.set_defaults(func=cmd_delete)

    return parser


def main(argv=None) -> None:
    args = build_parser().parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
