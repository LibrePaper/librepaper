"""SUPERSEDED. This validates a SQLite catalog v2 that was never shipped; the
storage that ships is PostgreSQL catalog v3, whose schema contract is exercised
by the postgres_v3_contract test in crates/librepaper/src/storage/postgres/.
Nothing in CI runs this file. Kept for the constraint reasoning it records.

Exercise the proposed DDL in memory; this does not migrate application data.

Run with Python 3.11+ linked to SQLite 3.37+:
    python3 docs/specs/validate-catalog-v2.py
These checks validate relational constraints and query plans, not application
protocols. Explicit boundary probes also show what SQL alone does NOT enforce.
"""

import sqlite3
from contextlib import contextmanager
from pathlib import Path


db = sqlite3.connect(":memory:", isolation_level=None)
db.execute("PRAGMA foreign_keys = ON")
schema = Path(__file__).with_name("catalog-v2.sql").read_text()
db.executescript("BEGIN IMMEDIATE;\n" + schema)
checks = 0
boundary_checks = 0


def check(condition, name):
    global checks
    if not condition:
        raise AssertionError(name)
    checks += 1


def insert_statement(table, values):
    columns = ",".join(values)
    placeholders = ",".join("?" for _ in values)
    return f"INSERT INTO {table} ({columns}) VALUES ({placeholders})", tuple(values.values())


def insert(table, values):
    db.execute(*insert_statement(table, values))


@contextmanager
def savepoint(name):
    """Leave the fixtures exactly as the probe found them: every check below
    reads the same rows, so none may be allowed to see an earlier one's write."""
    db.execute(f"SAVEPOINT {name}")
    try:
        yield
    finally:
        db.execute(f"ROLLBACK TO {name}")
        db.execute(f"RELEASE {name}")


def reject(name, sql, parameters=(), error_contains=None):
    global checks
    with savepoint("invalid_row"):
        try:
            db.execute(sql, parameters)
        except sqlite3.IntegrityError as error:
            if error_contains is not None and error_contains not in str(error):
                raise AssertionError(f"{name}: wrong rejection: {error}") from error
            checks += 1
        else:
            raise AssertionError(f"invalid state accepted: {name}")


def accept(name, sql, parameters=()):
    global checks
    with savepoint("valid_row"):
        try:
            db.execute(sql, parameters)
        except sqlite3.Error as error:
            raise AssertionError(f"valid state rejected: {name}: {error}") from error
        checks += 1


def boundary_probe(name, sql, parameters=()):
    """Document a SQL-permitted state that the typed application must forbid."""
    global boundary_checks
    with savepoint("application_boundary"):
        try:
            db.execute(sql, parameters)
        except sqlite3.Error as error:
            raise AssertionError(f"enforcement boundary changed: {name}: {error}") from error
        boundary_checks += 1


expected = {
    "accounts", "documents", "grants", "links", "annotations", "replies",
    "checkpoints", "objects", "checkpoint_objects", "object_leases",
    "operations", "server_state",
}
actual = {row[0] for row in db.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
check(actual == expected, "exact twelve-table inventory")
check(db.execute("SELECT count(*) FROM sqlite_schema WHERE type='trigger'").fetchone()[0] == 0,
      "no triggers")
strict = {row[1] for row in db.execute("PRAGMA table_list") if row[0] == "main" and row[5]}
check(strict >= expected, "all tables strict")
check(db.execute("PRAGMA user_version").fetchone()[0] == 0, "DDL alone does not initialize a deployment")
check(db.in_transaction, "schema creation remains in initialization transaction")

insert("accounts", dict(id="a", kind="registered", provider="github", provider_subject="1",
                        handle="alice", display_name="Alice", status="active", session_generation="g",
                        plan="standard", created_at=1000, last_seen_at=1000))
insert("server_state", dict(id=1, deployment_id="deployment", writer_generation="g",
                            active_link_key_id="key", keyring_json='{"version":1}',
                            cost_json='{"version":1}', updated_at=1000))
db.execute("PRAGMA user_version=2")
db.execute("COMMIT")
check(db.execute("PRAGMA user_version").fetchone()[0] == 2, "version 2 after initialization")
check(db.execute("SELECT count(*) FROM server_state").fetchone()[0] == 1, "initialized singleton")


def operation_values(op_id, document_id, kind="source_publish", **extra):
    values = dict(id=op_id, document_id=document_id, actor_key="actor", request_key=op_id,
                  kind=kind, request_digest="0" * 64, state="prepared", writer_generation="g",
                  created_at=1000, updated_at=1000, work_expires_at=2000)
    values.update(extra)
    return values


def operation(op_id, document_id, kind="source_publish", **extra):
    insert("operations", operation_values(op_id, document_id, kind, **extra))


for document in ("d1", "d2"):
    insert("documents", dict(id=document, slug=document, owner_id="a", ownership_mode="owned",
                             title=document, title_key=document, status="creating", created_at=1000,
                             updated_at=1000, source_format="markdown", main_path="main.md"))
    operation("op-" + document, document)
    insert("objects", dict(document_id=document, id="object-" + document,
                           storage_key="v2/" + document, kind="source_tree", state="allocated",
                           digest="0" * 64, reserved_bytes=12,
                           allocation_operation_id="op-" + document, created_at=1000))
    insert("object_leases", dict(document_id=document, object_id="object-" + document,
                                 holder_id="holder", purpose="write", operation_id="op-" + document,
                                 writer_generation="g", created_at=1000, expires_at=2000))
    db.execute("UPDATE objects SET state='available',byte_length=12,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?",
               (document,))
    insert("checkpoints", dict(document_id=document, id="checkpoint-" + document, seq=1,
                               tree_object_id="object-" + document, tree_digest="0" * 64,
                               created_at=1000, author_label="Alice", author_account_id="a", reason="test",
                               source_format="markdown", logical_bytes=12, journal_epoch=0, journal_sequence=0))
    insert("checkpoint_objects", dict(document_id=document, checkpoint_id="checkpoint-" + document,
                                      object_id="object-" + document))
    db.execute("UPDATE documents SET current_checkpoint_id=?,status='active' WHERE id=?",
               ("checkpoint-" + document, document))

reject("null text primary key", "UPDATE operations SET id=NULL WHERE id='op-d1'")
reject("text byte counter", "UPDATE documents SET stored_bytes='garbage' WHERE id='d1'")
reject("negative bytes", "UPDATE objects SET byte_length=-1 WHERE document_id='d1'")
reject("nonboolean root", "UPDATE objects SET live_root=7 WHERE document_id='d1'")
reject("double measured/reserved charge", "UPDATE objects SET reserved_bytes=1 WHERE document_id='d1'")
reject("deleting without retry deadline", "UPDATE objects SET state='deleting' WHERE document_id='d1'")
reject("invalid source format", "UPDATE documents SET source_format='unknown' WHERE id='d1'")
reject("invalid JSON", "UPDATE accounts SET preferences_json='broken' WHERE id='a'")
reject("provider identity absent", "UPDATE accounts SET provider_subject=NULL WHERE id='a'")
reject("negative sequence", "UPDATE checkpoints SET seq=-1 WHERE document_id='d1'")
reject("expired before creation", "UPDATE object_leases SET expires_at=999 WHERE document_id='d1'")
reject("nonjournal range fields", "UPDATE objects SET journal_epoch=0 WHERE document_id='d1'")
reject("foreign checkpoint", "UPDATE documents SET current_checkpoint_id='checkpoint-d2' WHERE id='d1'")
reject("foreign object", "INSERT INTO checkpoint_objects VALUES ('d1','checkpoint-d1','object-d2')")
reject("nonexistent checkpoint", "INSERT INTO checkpoint_objects VALUES ('d1','missing','object-d1')")
reject("available object cannot pin allocation operation", "UPDATE objects SET allocation_operation_id='op-d1' WHERE document_id='d1'")
reject("foreign allocation operation", """UPDATE objects SET state='allocated',byte_length=NULL,
       reserved_bytes=12,allocation_operation_id='op-d2' WHERE document_id='d1'""",
       error_contains="FOREIGN KEY constraint failed")
reject("foreign lease operation", "UPDATE object_leases SET operation_id='op-d2' WHERE document_id='d1'",
       error_contains="FOREIGN KEY constraint failed")
reject("delete current checkpoint", "DELETE FROM checkpoints WHERE document_id='d1'")
reject("delete referenced object", "DELETE FROM objects WHERE document_id='d1'")
reject("delete account with content", "DELETE FROM accounts WHERE id='a'")
reject("duplicate title", "UPDATE documents SET title_key='d1' WHERE id='d2'")
reject("multiple scopes", "UPDATE operations SET account_id='a' WHERE id='op-d1'")
reject("terminal operation without result", "UPDATE operations SET state='committed',completed_at=1000,receipt_expires_at=3000 WHERE id='op-d1'")
reject("two source writers", """INSERT INTO operations
       (id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,created_at,updated_at,work_expires_at)
       VALUES ('writer2','d1','actor','writer2','journal_append',?,'prepared','g',1000,1000,2000)""", ("0" * 64,),
       error_contains="UNIQUE constraint failed: operations.document_id")
operation("execution1", "d1", "agent_execution", conversation_id="conversation", execution_epoch="epoch",
          work_expires_at=61000)
reject("two active conversation epochs", """INSERT INTO operations
       (id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,
        created_at,updated_at,conversation_id,execution_epoch,work_expires_at)
       VALUES ('execution2','d1','actor','execution2','agent_execution',?,'prepared','g',
               1000,1000,'conversation','epoch2',61000)""", ("0" * 64,))
operation("rotation1", None, "rotate_links")
reject("two active link rotations", """INSERT INTO operations
       (id,actor_key,request_key,kind,request_digest,state,writer_generation,created_at,updated_at)
       VALUES ('rotation2','actor','rotation2','rotate_links',?,'prepared','g',1000,1000)""", ("0" * 64,))
insert("annotations", dict(document_id="d1", id="suggestion", seq=1, kind="suggestion", body="edit",
                               author_key="actor", author_label="Alice", via="test", created_at=1000,
                               updated_at=1000, selector_json='{"version":1}', proposed_text="new",
                               suggestion_state="proposed", protected_checkpoint_id="checkpoint-d1"))
reject("accepted without committed provenance", """UPDATE annotations SET suggestion_state='accepted'
       WHERE document_id='d1' AND id='suggestion'""")

# Scope and deadline checks must not pass accidentally because another
# uniqueness constraint happened to reject the fixture first.
document_kinds = {
    "source_publish", "display_publish", "checkpoint", "checkpoint_delete",
    "journal_append", "journal_compact", "agent_apply", "agent_annotations",
    "agent_cancel", "agent_execution", "agent_stage", "erase_document",
}
scope_by_kind = dict.fromkeys(document_kinds, "document")
scope_by_kind.update(erase_account="account", rotate_links="server", backup="server")
resumable = {"erase_account", "erase_document", "rotate_links", "backup", "journal_compact"}
for kind, required_scope in sorted(scope_by_kind.items()):
    for scope in ("document", "account", "server"):
        values = operation_values(
            "matrix-" + kind, "d1" if scope == "document" else None, kind,
            account_id="a" if scope == "account" else None, state="committed",
            result_json='{"version":1}', completed_at=1000, receipt_expires_at=3000,
            conversation_id="matrix-conversation", execution_epoch="matrix-" + kind,
            target_request_key="target",
        )
        action = accept if scope == required_scope else reject
        action(f"{kind}: {scope} scope", *insert_statement("operations", values))

    # Temporarily settle existing jobs so each correct prepared fixture can
    # exercise its deadline independently of active-job uniqueness indexes.
    with savepoint("deadline_matrix"):
        db.execute("""UPDATE operations SET state='aborted',completed_at=1000,
                   result_json='{}',receipt_expires_at=3000 WHERE state='prepared'""")
        values = operation_values(
            "deadline-" + kind, "d1" if required_scope == "document" else None, kind,
            account_id="a" if required_scope == "account" else None,
            work_expires_at=None, conversation_id="deadline-conversation",
            execution_epoch="deadline-" + kind, target_request_key="target",
        )
        action = accept if kind in resumable else reject
        action(f"{kind}: NULL prepared deadline", *insert_statement("operations", values))
        values["work_expires_at"] = 2000
        accept(f"{kind}: finite prepared deadline", *insert_statement("operations", values))
        for terminal in ("committed", "aborted"):
            terminal_values = dict(values, state=terminal, result_json="{}", completed_at=1000,
                                   receipt_expires_at=None)
            reject(f"{kind}: {terminal} requires receipt expiry",
                   *insert_statement("operations", terminal_values))

reject("receipt cannot expire before completion", """UPDATE operations SET state='committed',
       completed_at=2000,updated_at=2000,result_json='{}',receipt_expires_at=1500 WHERE id='op-d1'""")

for op_id, doc_id, kind, account_id, key_column in (
    ("erase-account", None, "erase_account", "a", "account_id"),
    ("erase-document", "d2", "erase_document", None, "document_id"),
):
    values = operation_values(op_id, doc_id, kind, account_id=account_id, work_expires_at=None)
    insert("operations", values)
    duplicate = dict(values, id=op_id + "-again", request_key=op_id + "-again")
    reject("one erasure per " + key_column, *insert_statement("operations", duplicate),
           error_contains="UNIQUE constraint failed: operations." + key_column)

# Each natural replay query must use the corresponding unique index without
# hints or COALESCE spellings. The same key is allowed in different scopes.
for scope, document_id, account_id, kind, predicate, parameters in (
    ("document", "d1", None, "checkpoint_delete", "document_id=? AND actor_key=? AND request_key=?",
     ("d1", "actor", "shared-key")),
    ("account", None, "a", "erase_account", "account_id=? AND actor_key=? AND request_key=?",
     ("a", "actor", "shared-key")),
    ("server", None, None, "backup", "document_id IS NULL AND account_id IS NULL AND actor_key=? AND request_key=?",
     ("actor", "shared-key")),
):
    values = operation_values("replay-" + scope, document_id, kind, account_id=account_id,
                              request_key="shared-key", state="committed", completed_at=1000,
                              result_json="{}", receipt_expires_at=3000)
    insert("operations", values)
    query = "SELECT id FROM operations WHERE " + predicate
    check(db.execute(query, parameters).fetchall() == [(values["id"],)], scope + " replay result")
    plan = " ".join(row[3] for row in db.execute("EXPLAIN QUERY PLAN " + query, parameters))
    check("operations_request_" + scope in plan, scope + " replay index: " + plan)
    reject(scope + " duplicate request", *insert_statement("operations", dict(values, id=values["id"] + "-dup")),
           error_contains="UNIQUE constraint failed")
    accept(scope + " different actor", *insert_statement("operations", dict(values, id=values["id"] + "-other",
                                                                                           actor_key="other-actor")))

for table, key, limits in (
    ("documents", "d1", {"agent_payload_bytes": 33554432, "agent_payload_count": 512,
                          "checkpoint_ref_count": 1048576}),
    ("server_state", 1, {"agent_payload_bytes": 134217728, "agent_payload_count": 16384,
                         "checkpoint_ref_count": 8388608}),
):
    for column, limit in limits.items():
        query = f"UPDATE {table} SET {column}=? WHERE id=?"
        accept(f"{table}.{column} at ceiling", query, (limit, key))
        reject(f"{table}.{column} over ceiling", query, (limit + 1, key))
        reject(f"{table}.{column} negative", query, (-1, key))
    query = f"SELECT agent_payload_bytes,agent_payload_count,checkpoint_ref_count FROM {table} WHERE id=?"
    plan = " ".join(row[3] for row in db.execute("EXPLAIN QUERY PLAN " + query, (key,)))
    check("SEARCH " + table in plan and "SCAN" not in plan, table + " counter lookup: " + plan)

base_object = dict(document_id="d1", id="empty", storage_key="v2/empty", kind="agent_payload",
                   state="deleting", digest="0" * 64, reserved_bytes=0, created_at=1000, retry_at=2000)
reject("unknown deleting allocation needs owner", *insert_statement("objects", base_object))
accept("confirmed empty object can be deleting", *insert_statement("objects", dict(base_object, byte_length=0)))
accept("unknown empty allocation retains its operation", *insert_statement("objects", dict(base_object,
                                                                                            allocation_operation_id="op-d1")))

# Available objects no longer pin allocation receipts; an actual staging lease
# still does. Test that cleanup succeeds only after the lease is removed.
operation("stage-cleanup", "d2", "agent_stage")
insert("objects", dict(base_object, document_id="d2", id="staged", storage_key="v2/staged",
                       state="available", byte_length=0, retry_at=None))
insert("object_leases", dict(document_id="d2", object_id="staged", holder_id="stage-holder",
                             purpose="stage", operation_id="stage-cleanup", writer_generation="g",
                             created_at=1000, expires_at=2000))
db.execute("""UPDATE operations SET state='aborted',completed_at=1000,result_json='{}',receipt_expires_at=3000
           WHERE id='stage-cleanup'""")
reject("staging lease pins its receipt", "DELETE FROM operations WHERE id='stage-cleanup'",
       error_contains="FOREIGN KEY constraint failed")
db.execute("DELETE FROM object_leases WHERE holder_id='stage-holder'")
db.execute("DELETE FROM operations WHERE id='stage-cleanup'")
check(db.execute("SELECT state FROM objects WHERE document_id='d2' AND id='staged'").fetchone() == ("available",),
      "settled object survives receipt cleanup")

with savepoint("title_release"):
    db.execute("UPDATE documents SET status='deleting' WHERE id='d1'")
    accept("deleting document releases title", "UPDATE documents SET title_key='d1' WHERE id='d2'")
    reject("deleting document retains slug", "UPDATE documents SET slug='d1' WHERE id='d2'",
           error_contains="UNIQUE constraint failed: documents.slug")

# These probes intentionally succeed in raw SQL. They are NOT valid product
# operations and require the preventative typed-transaction tests in the spec.
incomplete = dict(document_id="d1", id="incomplete", seq=999, tree_object_id="object-d1",
                  tree_digest="0" * 64, created_at=1000, author_label="Alice", reason="probe",
                  source_format="markdown", logical_bytes=12, journal_epoch=0, journal_sequence=0)
boundary_probe("checkpoint with no closure", *insert_statement("checkpoints", incomplete))
for state in ("allocated", "deleting"):
    values = dict(base_object, id="boundary-" + state, storage_key="v2/boundary-" + state, state=state)
    if state == "allocated":
        values.update(reserved_bytes=12, allocation_operation_id="op-d1", retry_at=None)
    else:
        values["byte_length"] = 12
    insert("objects", values)
    boundary_probe("closure points at " + state, "INSERT INTO checkpoint_objects VALUES (?,?,?)",
                   ("d1", "checkpoint-d1", values["id"]))
boundary_probe("read lease on deleting object", """INSERT INTO object_leases
               (document_id,object_id,holder_id,purpose,writer_generation,created_at,expires_at)
               VALUES ('d1','boundary-deleting','raw-reader','read','g',1000,2000)""")

# Demonstrate that rollback includes DDL, the singleton, and user_version.
failed_init = sqlite3.connect(":memory:", isolation_level=None)
failed_init.execute("PRAGMA foreign_keys=ON")
failed_init.executescript("BEGIN IMMEDIATE;\n" + schema)
failed_init.execute(*insert_statement("server_state", dict(id=1, deployment_id="failed", writer_generation="g",
                                                          active_link_key_id="key", keyring_json="{}", cost_json="{}",
                                                          updated_at=1000)))
failed_init.execute("PRAGMA user_version=2")
failed_init.execute("ROLLBACK")
check(failed_init.execute("PRAGMA user_version").fetchone()[0] == 0, "failed init rolls back version")
check(failed_init.execute("SELECT name FROM sqlite_schema WHERE type='table'").fetchall() == [],
      "failed init rolls back schema and singleton")
failed_init.close()

check(db.execute("PRAGMA foreign_key_check").fetchall() == [], "no foreign key violations")
check(db.execute("PRAGMA integrity_check").fetchone()[0] == "ok", "integrity check")
print(f"{checks} schema checks passed on SQLite {sqlite3.sqlite_version}; 12 tables, 0 triggers.")
print(f"{boundary_checks} SQL-permitted boundary cases confirmed; the application must reject them transactionally.")
print("No application database was opened or changed. Runtime protocols require separate implementation tests.")
