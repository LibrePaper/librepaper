#!/usr/bin/env python3
"""Measure the v2 SQL reference matrix in disposable catalogs.

This measures SQL storage and transaction cost, not the Rust transaction API,
physical object I/O, manifest validation, or crash recovery. Run without other
builds/tests for comparable timings. No application database is accepted.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import sqlite3
import statistics
import tempfile
import time


MATRIX = {
    "small": (100, 50, 128),
    "histories": (1000, 50, 128),
    "documents": (10000, 10, 64),
    "document-ceiling": (1, 64, 16384),
    "deployment-ceiling": (8, 64, 16384),
}
SCHEMA = Path(__file__).with_name("catalog-v2.sql").read_text()
EMPTY_DIGEST = hashlib.sha256(b"").hexdigest()


def distribution(values):
    ordered = sorted(values)
    return {
        "samples": len(values), "mean_ms": statistics.mean(values),
        "p95_ms": ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))],
        "max_ms": max(values),
    }


def measured(db, sql, args=()):
    # Python's API exposes a progress callback, not sqlite3_stmt_status.
    # Thus report a VM-step interval, not an exact statement instruction count.
    callbacks = 0

    def progress():
        nonlocal callbacks
        callbacks += 1
        return 0

    db.set_progress_handler(progress, 100)
    before = time.perf_counter()
    rows = db.execute(sql, args).fetchall()
    elapsed = (time.perf_counter() - before) * 1000
    db.set_progress_handler(None, 0)
    return {"wall_ms": elapsed, "vm_steps_interval": [callbacks * 100, callbacks * 100 + 99],
            "rows": len(rows), "plan": [r[3] for r in db.execute("EXPLAIN QUERY PLAN " + sql, args)]}


def workload(name, reuse, parent):
    documents, checkpoints, edges = MATRIX[name]
    new_per_checkpoint = edges - edges * reuse // 100
    with tempfile.TemporaryDirectory(prefix="catalog-v2-bench-", dir=parent) as temporary:
        path = Path(temporary) / "synthetic.sqlite"
        db = sqlite3.connect(path, isolation_level=None)
        db.execute("PRAGMA foreign_keys=ON")
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("PRAGMA synchronous=FULL")
        db.executescript("BEGIN IMMEDIATE;\n" + SCHEMA)
        db.execute("INSERT INTO server_state(id,deployment_id,writer_generation,active_link_key_id,keyring_json,cost_json,updated_at) VALUES(1,?,'benchmark','benchmark','{}','{}',0)", ("b" * 64,))
        db.execute("INSERT INTO accounts(id,kind,handle,display_name,status,session_generation,plan,created_at,last_seen_at) VALUES('benchmark','system','benchmark','Benchmark','active','benchmark','benchmark',0,0)")
        db.execute("PRAGMA user_version=2")
        db.execute("COMMIT")
        checkpoint_times = []
        max_wal = 0
        objects = 0
        before_all = time.perf_counter()
        for doc_index in range(documents):
            document = f"{doc_index:032x}"
            db.execute("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path,retention_mode) VALUES(?1,?1,'benchmark','owned',?1,?1,'active',0,0,'markdown','index.md','manual')", (document,))
            previous = []
            next_object = 0
            for seq in range(1, checkpoints + 1):
                count = edges if seq == 1 else new_per_checkpoint
                fresh = [f"{i:032x}" for i in range(next_object, next_object + count)]
                next_object += count
                current = fresh + previous[:edges - count]
                checkpoint = f"{seq:032x}"
                start = time.perf_counter()
                db.execute("BEGIN IMMEDIATE")
                # Distinct physical IDs remain distinct even for equal empty
                # bytes. The reuse parameter controls IDs, not byte content.
                db.executemany("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at,gc_after) VALUES(?,?,?,'source_tree','available',?,0,0,0,0)",
                               ((document, oid, f"v2/documents/{document}/objects/{oid}", EMPTY_DIGEST) for oid in fresh))
                db.execute("INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,created_at,author_label,reason,source_format,logical_bytes,journal_epoch,journal_sequence) VALUES(?,?,?,?,?,0,'Benchmark','automatic','markdown',0,0,0)",
                           (document, checkpoint, seq, current[0], EMPTY_DIGEST))
                db.executemany("INSERT INTO checkpoint_objects VALUES(?,?,?)", ((document, checkpoint, oid) for oid in current))
                db.execute("UPDATE documents SET current_checkpoint_id=?,next_checkpoint_seq=?,checkpoint_ref_count=checkpoint_ref_count+? WHERE id=?", (checkpoint, seq + 1, edges, document))
                db.execute("UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count+? WHERE id=1", (edges,))
                db.execute("COMMIT")
                checkpoint_times.append((time.perf_counter() - start) * 1000)
                previous = current
                objects += count
            max_wal = max(max_wal, Path(str(path) + "-wal").stat().st_size)
        db.execute("UPDATE accounts SET document_count=?", (documents,))
        db.execute("UPDATE server_state SET document_count=?", (documents,))
        build_seconds = time.perf_counter() - before_all
        expected = documents * checkpoints * edges
        actual = db.execute("SELECT count(*) FROM checkpoint_objects").fetchone()[0]
        assert actual == expected
        assert db.execute("SELECT sum(checkpoint_ref_count) FROM documents").fetchone()[0] == expected
        assert db.execute("SELECT checkpoint_ref_count FROM server_state").fetchone()[0] == expected
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        db.execute("ANALYZE")
        probes = {
            "quota_rows": measured(db, "SELECT d.reserved_bytes,a.stored_bytes,a.reserved_bytes,s.stored_bytes,s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s WHERE d.id=? AND s.id=1", ("0" * 32,)),
            "reference_count_rows": measured(db, "SELECT d.checkpoint_ref_count,s.checkpoint_ref_count FROM documents d CROSS JOIN server_state s WHERE d.id=? AND s.id=1", ("0" * 32,)),
            "gc_candidates": measured(db, "SELECT id FROM objects WHERE state='available' AND live_root=0 AND publication_root=0 AND gc_after<=0 ORDER BY gc_after,document_id,id LIMIT 256"),
            "live_root_selection": measured(db, "SELECT id FROM objects INDEXED BY objects_live_roots WHERE document_id=? AND state='available' AND live_root=1 AND kind='source_tree'", ("0" * 32,)),
        }
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        catalog_bytes = path.stat().st_size
        delete_count = min(32, 32768 // edges, checkpoints - 1)
        start = time.perf_counter()
        db.execute("BEGIN IMMEDIATE")
        for seq in range(1, delete_count + 1):
            db.execute("DELETE FROM checkpoint_objects WHERE document_id=? AND checkpoint_id=?", ("0" * 32, f"{seq:032x}"))
            db.execute("DELETE FROM checkpoints WHERE document_id=? AND id=?", ("0" * 32, f"{seq:032x}"))
        db.execute("UPDATE documents SET checkpoint_ref_count=checkpoint_ref_count-? WHERE id=?", (delete_count * edges, "0" * 32))
        db.execute("UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count-? WHERE id=1", (delete_count * edges,))
        db.execute("COMMIT")
        deletion_ms = (time.perf_counter() - start) * 1000
        result = {"name": name, "reuse_percent": reuse, "documents": documents,
                  "checkpoints_per_document": checkpoints, "edges_per_checkpoint": edges,
                  "reference_rows_before_deletion": actual, "distinct_object_rows": objects,
                  "prepared_operations": 0, "terminal_operations": 0,
                  "catalog_bytes_including_indexes": catalog_bytes,
                  "sampled_max_wal_bytes": max_wal,
                  "build_seconds": build_seconds, "checkpoint_transaction": distribution(checkpoint_times),
                  "bounded_deletion": {"checkpoints": delete_count, "edges": delete_count * edges, "wall_ms": deletion_ms},
                  "probes": probes}
        db.close()
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workload", choices=[*MATRIX, "all"], default="small")
    parser.add_argument("--reuse", type=int, choices=[0, 90, 100], default=90)
    parser.add_argument("--temporary-parent", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output exists; choose a new measurement file")
    cases = [(args.workload, args.reuse)] if args.workload != "all" else [
        (name, reuse) for name in MATRIX
        for reuse in ([0, 90, 100] if name in ("small", "histories", "documents") else [100])]
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown") if Path("/proc/cpuinfo").exists() else platform.processor()
    report = {"scope": "SQL-only synthetic reference storage; not application protocol acceptance",
              "complete": False, "started_at": datetime.now(timezone.utc).isoformat(),
              "schema_sha256": hashlib.sha256(SCHEMA.encode()).hexdigest(),
              "requested_cases": [{"workload": name, "reuse_percent": reuse} for name, reuse in cases],
              "sqlite_version": sqlite3.sqlite_version, "python_version": platform.python_version(),
              "platform": platform.platform(), "cpu": cpu, "cpu_count": os.cpu_count(),
              "journal_mode": "WAL", "synchronous": "FULL", "results": []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        json.dump(report, output, indent=2)
        output.flush()
        for name, reuse in cases:
            print(f"Measuring {name}, {reuse}% reuse", flush=True)
            report["results"].append(workload(name, reuse, args.temporary_parent))
            output.seek(0)
            json.dump(report, output, indent=2)
            output.truncate()
            output.flush()
        report["complete"] = True
        report["finished_at"] = datetime.now(timezone.utc).isoformat()
        output.seek(0)
        json.dump(report, output, indent=2)
        output.truncate()
    print(f"Wrote {args.output}", flush=True)


if __name__ == "__main__":
    main()
