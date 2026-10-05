#!/usr/bin/env python3
"""Persistent, identifier-free backup job status and node-exporter metrics."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import tempfile
import time
from pathlib import Path
from typing import Any

STATE_PATH = Path(os.environ.get("LIBREPAPER_BACKUP_STATE", "/var/backups/librepaper/.status-state.json"))
METRICS_PATH = Path(os.environ.get("LIBREPAPER_BACKUP_METRICS", "/var/backups/librepaper/metrics/librepaper_backup.prom"))
JOBS = ("backup", "check")


def _atomic_write(path: Path, content: str, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _empty_state() -> dict[str, Any]:
    return {
        "enabled": 0,
        "config_error": 0,
        "startup_timestamp_seconds": 0,
        "first_enabled_timestamp_seconds": 0,
        "jobs": {
            job: {
                "started_timestamp_seconds": 0,
                "completed_timestamp_seconds": 0,
                "success_timestamp_seconds": 0,
                "failure_timestamp_seconds": 0,
                "last_result_success": -1,
            }
            for job in JOBS
        },
    }


def _read_state() -> dict[str, Any]:
    try:
        state = json.loads(STATE_PATH.read_text(encoding="utf-8"))
        if not isinstance(state, dict) or not isinstance(state.get("jobs"), dict):
            raise ValueError("invalid state")
        for field in ("enabled", "config_error", "startup_timestamp_seconds", "first_enabled_timestamp_seconds"):
            if type(state.get(field)) is not int or state[field] < 0:
                raise ValueError("invalid state field")
        if state["enabled"] not in (0, 1) or state["config_error"] not in (0, 1):
            raise ValueError("invalid state flag")
        for job in JOBS:
            record = state["jobs"].get(job)
            if not isinstance(record, dict):
                raise ValueError("invalid job state")
            for field in ("started_timestamp_seconds", "completed_timestamp_seconds", "success_timestamp_seconds", "failure_timestamp_seconds"):
                if type(record.get(field)) is not int or record[field] < 0:
                    raise ValueError("invalid job timestamp")
            if type(record.get("last_result_success")) is not int or record["last_result_success"] not in (-1, 0, 1):
                raise ValueError("invalid job result")
        return state
    except FileNotFoundError:
        if METRICS_PATH.exists():
            raise ValueError("private status state is missing")
        return _empty_state()
    except (json.JSONDecodeError, UnicodeDecodeError, TypeError, KeyError) as error:
        raise ValueError("invalid private status state") from error


def _metrics(state: dict[str, Any]) -> str:
    lines = [
        "# HELP librepaper_backup_enabled Whether backups are enabled by configuration.",
        "# TYPE librepaper_backup_enabled gauge",
        f"librepaper_backup_enabled {int(state['enabled'])}",
        "# HELP librepaper_backup_config_error Whether startup rejected the backup configuration.",
        "# TYPE librepaper_backup_config_error gauge",
        f"librepaper_backup_config_error {int(state['config_error'])}",
        "# HELP librepaper_backup_state_error Whether persistent status state could not be read or updated.",
        "# TYPE librepaper_backup_state_error gauge",
        "librepaper_backup_state_error 0",
        "# HELP librepaper_backup_startup_timestamp_seconds Most recent sidecar startup time.",
        "# TYPE librepaper_backup_startup_timestamp_seconds gauge",
        f"librepaper_backup_startup_timestamp_seconds {int(state['startup_timestamp_seconds'])}",
        "# HELP librepaper_backup_first_enabled_timestamp_seconds First time backup configuration was enabled.",
        "# TYPE librepaper_backup_first_enabled_timestamp_seconds gauge",
        f"librepaper_backup_first_enabled_timestamp_seconds {int(state['first_enabled_timestamp_seconds'])}",
    ]
    metric_names = (
        ("started_timestamp_seconds", "job_started_timestamp_seconds"),
        ("completed_timestamp_seconds", "job_completed_timestamp_seconds"),
        ("success_timestamp_seconds", "job_last_success_timestamp_seconds"),
        ("failure_timestamp_seconds", "job_last_failure_timestamp_seconds"),
        ("last_result_success", "job_last_result_success"),
    )
    for _, metric in metric_names:
        lines.extend((f"# TYPE librepaper_backup_{metric} gauge",))
    for job in JOBS:
        record = state["jobs"][job]
        for field, metric in metric_names:
            lines.append(f'librepaper_backup_{metric}{{task="{job}"}} {int(record[field])}')
    return "\n".join(lines) + "\n"


def _write(state: dict[str, Any]) -> None:
    # Private durable state is separate from the scheduler's private tmpfs.
    _atomic_write(STATE_PATH, json.dumps(state, sort_keys=True) + "\n", 0o600)
    # node-exporter runs under another UID and reads only this non-sensitive file.
    _atomic_write(METRICS_PATH, _metrics(state), 0o644)
    os.chmod(METRICS_PATH.parent, 0o755)


def _publish_state_error() -> None:
    """Expose corruption without replacing the last known successful history."""
    try:
        existing = METRICS_PATH.read_text(encoding="utf-8")
    except (FileNotFoundError, OSError, UnicodeDecodeError):
        existing = ""
    lines = [line for line in existing.splitlines() if not line.startswith("librepaper_backup_state_error") and not line.startswith("# HELP librepaper_backup_state_error") and not line.startswith("# TYPE librepaper_backup_state_error")]
    lines.extend(("# HELP librepaper_backup_state_error Whether persistent status state could not be read or updated.", "# TYPE librepaper_backup_state_error gauge", "librepaper_backup_state_error 1"))
    try:
        _atomic_write(METRICS_PATH, "\n".join(lines) + "\n", 0o644)
        os.chmod(METRICS_PATH.parent, 0o755)
    except OSError:
        pass


def update(event: str, job: str | None = None) -> None:
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    lock_path = STATE_PATH.with_suffix(STATE_PATH.suffix + ".lock")
    with lock_path.open("a", encoding="utf-8") as lock:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = _read_state()
        now = int(time.time())
        if event == "config-error":
            state["config_error"] = 1
            state["startup_timestamp_seconds"] = now
        elif event in ("enabled", "disabled"):
            state["enabled"] = int(event == "enabled")
            state["config_error"] = 0
            state["startup_timestamp_seconds"] = now
            if event == "enabled" and not state["first_enabled_timestamp_seconds"]:
                state["first_enabled_timestamp_seconds"] = now
        elif event in ("start", "success", "failure") and job in JOBS:
            record = state["jobs"][job]
            if event == "start":
                record["started_timestamp_seconds"] = now
                record["last_result_success"] = -1
            else:
                record["completed_timestamp_seconds"] = now
                record["last_result_success"] = int(event == "success")
                record[f"{event}_timestamp_seconds"] = now
        else:
            raise ValueError("invalid status operation")
        _write(state)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("event", choices=("enabled", "disabled", "config-error", "start", "success", "failure"))
    parser.add_argument("job", nargs="?", choices=JOBS)
    args = parser.parse_args()
    if args.event in ("enabled", "disabled", "config-error") and args.job is not None:
        parser.error("configuration events take no job")
    if args.event in ("start", "success", "failure") and args.job is None:
        parser.error("job is required for job events")
    try:
        update(args.event, args.job)
    except (OSError, ValueError, json.JSONDecodeError):
        # Never echo config or state contents to logs.
        _publish_state_error()
        print("backup status update failed", file=__import__("sys").stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
