"""Pytest suite for the todo CLI (todo.py)."""

import json

import pytest

import todo


@pytest.fixture
def store(tmp_path):
    """Path to a fresh (non-existent) todo store file."""
    return tmp_path / "todos.json"


@pytest.fixture
def run(store):
    """Return a callable that invokes todo.main against the store file."""

    def _run(*args):
        todo.main(["--file", str(store), *args])

    return _run


def read_store(store):
    return json.loads(store.read_text())


# ---------------------------------------------------------------- add

def test_add_stores_text_and_id(run, store):
    run("add", "buy milk")
    data = read_store(store)
    assert data["todos"] == [{"id": 1, "text": "buy milk", "done": False}]


def test_add_increments_next_id(run, store):
    run("add", "first")
    run("add", "second")
    data = read_store(store)
    assert data["next_id"] == 3
    assert [t["id"] for t in data["todos"]] == [1, 2]


def test_add_prints_confirmation(run, capsys):
    run("add", "hello")
    assert "Added todo 1: hello" in capsys.readouterr().out


# ---------------------------------------------------------------- list

def test_list_empty_store(run, capsys):
    run("list")
    assert capsys.readouterr().out.strip() == "No todos."


def test_list_pending_formatting(run, capsys):
    run("add", "pending task")
    capsys.readouterr()
    run("list")
    assert capsys.readouterr().out.strip() == "[ ] 1: pending task"


def test_list_done_formatting(run, capsys):
    run("add", "finished task")
    run("done", "1")
    capsys.readouterr()
    run("list")
    assert capsys.readouterr().out.strip() == "[x] 1: finished task"


def test_list_multiple_in_order(run, capsys):
    run("add", "a")
    run("add", "b")
    run("add", "c")
    capsys.readouterr()
    run("list")
    assert capsys.readouterr().out.strip().splitlines() == [
        "[ ] 1: a",
        "[ ] 2: b",
        "[ ] 3: c",
    ]


# ---------------------------------------------------------------- done

def test_done_marks_todo(run, store):
    run("add", "task")
    run("done", "1")
    assert read_store(store)["todos"][0]["done"] is True


def test_done_is_idempotent(run):
    run("add", "task")
    run("done", "1")
    run("done", "1")  # no error, stays done


def test_done_bad_id_exits_1_with_stderr(run, capsys):
    with pytest.raises(SystemExit) as exc:
        run("done", "99")
    assert exc.value.code == 1
    assert "99" in capsys.readouterr().err


# ---------------------------------------------------------------- delete

def test_delete_removes_entry(run, store):
    run("add", "keep")
    run("add", "remove")
    run("delete", "2")
    data = read_store(store)
    assert data["todos"] == [{"id": 1, "text": "keep", "done": False}]


def test_delete_bad_id_exits_1_with_stderr(run, capsys):
    with pytest.raises(SystemExit) as exc:
        run("delete", "99")
    assert exc.value.code == 1
    assert "99" in capsys.readouterr().err


# ---------------------------------------------------------------- persistence

def test_data_persists_across_calls(run, store, capsys):
    run("add", "durable task")
    capsys.readouterr()
    run("list")  # separate main() call
    assert "1: durable task" in capsys.readouterr().out


def test_on_disk_json_structure(run, store):
    run("add", "persist me")
    run("done", "1")
    data = read_store(store)
    assert set(data.keys()) == {"next_id", "todos"}
    assert isinstance(data["next_id"], int)
    assert isinstance(data["todos"], list)
    entry = data["todos"][0]
    assert set(entry.keys()) == {"id", "text", "done"}
    assert entry == {"id": 1, "text": "persist me", "done": True}
