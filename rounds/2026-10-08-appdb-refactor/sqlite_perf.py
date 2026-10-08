"""SQLite cold-open and CRUD comparison for the appdb perf workload."""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path


PAYLOAD = "x" * 256


def now_ms() -> float:
    return time.perf_counter() * 1000.0


def rows(prefix: str, start: int, count: int, version: int) -> list[tuple[str, int, str, int]]:
    return [(f"{prefix}-{sequence:08}", sequence, PAYLOAD, version) for sequence in range(start, start + count)]


def storage_stats(path: Path, depth: int = 0) -> tuple[int, int]:
    if depth >= 5 or not path.exists():
        return (0, 0)
    if path.is_file():
        return (path.stat().st_size, 1)
    total_bytes = 0
    total_files = 0
    for entry in path.iterdir():
        if entry.is_dir():
            nested_bytes, nested_files = storage_stats(entry, depth + 1)
            total_bytes += nested_bytes
            total_files += nested_files
        else:
            total_bytes += entry.stat().st_size
            total_files += 1
    return (total_bytes, total_files)


def open_db(path: Path, create_schema: bool) -> tuple[sqlite3.Connection, float]:
    started = now_ms()
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA journal_mode=DELETE")
    connection.execute("PRAGMA synchronous=FULL")
    connection.execute("PRAGMA temp_store=MEMORY")
    if create_schema:
        connection.executescript(
            """
            CREATE TABLE IF NOT EXISTS perf_load_row (
                id TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL,
                payload TEXT NOT NULL,
                version INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS perf_load_row_sequence_id_pagin
                ON perf_load_row(sequence, id);
            """
        )
        connection.commit()
    return connection, now_ms() - started


def measure_reads(connection: sqlite3.Connection, sample_id: str, expected_count: int) -> dict[str, float | int]:
    started = now_ms()
    page = connection.execute(
        "SELECT id, sequence, payload, version FROM perf_load_row ORDER BY sequence ASC, id ASC LIMIT 101"
    ).fetchall()
    page_ms = now_ms() - started
    if not page:
        raise RuntimeError("SQLite page was empty")

    started = now_ms()
    row = connection.execute(
        "SELECT id, sequence, payload, version FROM perf_load_row WHERE id = ?",
        (sample_id,),
    ).fetchone()
    get_ms = now_ms() - started
    if row is None:
        raise RuntimeError(f"SQLite sample row was missing: {sample_id}")

    started = now_ms()
    limited = connection.execute(
        "SELECT id, sequence, payload, version FROM perf_load_row LIMIT 100"
    ).fetchall()
    list_limit_ms = now_ms() - started
    if len(limited) != 100:
        raise RuntimeError("SQLite bounded list returned the wrong count")

    started = now_ms()
    all_rows = connection.execute(
        "SELECT id, sequence, payload, version FROM perf_load_row"
    ).fetchall()
    list_all_ms = now_ms() - started
    if len(all_rows) != expected_count:
        raise RuntimeError("SQLite full list returned the wrong count")

    return {
        "page_ms": round(page_ms, 3),
        "get_ms": round(get_ms, 3),
        "list_limit_ms": round(list_limit_ms, 3),
        "list_all_ms": round(list_all_ms, 3),
        "loaded_rows": len(all_rows),
    }


def child(mode: str, path: Path, count: int, delete_count: int, rounds_count: int) -> None:
    connection, open_ms = open_db(path, mode == "seed")
    try:
        if mode == "seed":
            started = now_ms()
            connection.executemany(
                "INSERT INTO perf_load_row(id, sequence, payload, version) VALUES (?, ?, ?, ?)",
                rows("baseline", 0, count, 0),
            )
            connection.commit()
            bytes_used, files = storage_stats(path)
            print(json.dumps({
                "scenario": "seed",
                "rows": count,
                "open_ms": round(open_ms, 3),
                "write_ms": round(now_ms() - started, 3),
                "bytes": bytes_used,
                "files": files,
            }))
            return

        if mode in ("read_baseline", "read_post_churn"):
            sample_id = f"baseline-{count - 1:08}" if mode == "read_baseline" else f"churn-{count:08}"
            reads = measure_reads(connection, sample_id, count)
            bytes_used, files = storage_stats(path)
            print(json.dumps({
                "scenario": mode,
                "rows": count,
                "open_ms": round(open_ms, 3),
                **reads,
                "bytes": bytes_used,
                "files": files,
            }))
            return

        if mode != "churn":
            raise RuntimeError(f"unknown mode: {mode}")

        started = now_ms()
        current = connection.execute("SELECT id, sequence, payload, version FROM perf_load_row").fetchall()
        if len(current) != count:
            raise RuntimeError("SQLite churn source returned the wrong count")
        load_all_ms = now_ms() - started

        started = now_ms()
        for round_number in range(1, rounds_count + 1):
            connection.executemany(
                "UPDATE perf_load_row SET version = ? WHERE id = ?",
                ((round_number, row[0]) for row in current),
            )
            removed = current[:delete_count]
            connection.executemany(
                "DELETE FROM perf_load_row WHERE id = ?",
                ((row[0],) for row in removed),
            )
            inserted = rows("churn", count + (round_number - 1) * delete_count, delete_count, round_number)
            connection.executemany(
                "INSERT INTO perf_load_row(id, sequence, payload, version) VALUES (?, ?, ?, ?)",
                inserted,
            )
            current = current[delete_count:] + inserted
        connection.commit()
        bytes_used, files = storage_stats(path)
        print(json.dumps({
            "scenario": "churn",
            "rows": count,
            "delete_rows_per_round": delete_count,
            "churn_rounds": rounds_count,
            "open_ms": round(open_ms, 3),
            "load_all_ms": round(load_all_ms, 3),
            "churn_ms": round(now_ms() - started, 3),
            "bytes": bytes_used,
            "files": files,
        }))
    finally:
        connection.close()


def run_scenario(count: int, delete_count: int, rounds_count: int) -> dict[str, object]:
    path = Path(tempfile.mkdtemp(prefix="appdb_sqlite_perf_")) / "perf.db"
    try:
        metrics: dict[str, object] = {
            "scenario": "sqlite_load_speed",
            "rows": count,
            "delete_rows_per_round": delete_count,
            "churn_rounds": rounds_count,
        }
        for mode in ("seed", "read_baseline", "churn", "read_post_churn"):
            output = subprocess.check_output(
                [sys.executable, __file__, mode, str(path), str(count), str(delete_count), str(rounds_count)],
                text=True,
            )
            value = json.loads(output)
            prefix = value["scenario"]
            for key, item in value.items():
                if key not in ("scenario", "rows"):
                    metrics[f"{prefix}_{key}"] = item
        return metrics
    finally:
        shutil.rmtree(path.parent, ignore_errors=True)


def main() -> None:
    if len(sys.argv) == 1:
        count = int(os.environ.get("APPDB_PERF_ROWS", "20000"))
        delete_count = int(os.environ.get("APPDB_PERF_DELETE_ROWS", str(count // 4)))
        rounds_count = int(os.environ.get("APPDB_PERF_CHURN_ROUNDS", "3"))
        print(json.dumps(run_scenario(count, delete_count, rounds_count)))
        return
    child(sys.argv[1], Path(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]))


if __name__ == "__main__":
    main()
