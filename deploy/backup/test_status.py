"""Focused tests for durable sidecar state, hook events, and startup validation."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


STATUS_FILE = Path(__file__).with_name("status.py")
ENTRYPOINT = Path(__file__).with_name("entrypoint.sh")
SPEC = importlib.util.spec_from_file_location("backup_status", STATUS_FILE)
assert SPEC is not None and SPEC.loader is not None
status = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(status)


class StatusStateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.state_path = status.STATE_PATH
        self.metrics_path = status.METRICS_PATH
        status.STATE_PATH = self.root / "private" / "state.json"
        status.METRICS_PATH = self.root / "metrics" / "backup.prom"

    def tearDown(self) -> None:
        status.STATE_PATH = self.state_path
        status.METRICS_PATH = self.metrics_path
        self.temp.cleanup()

    def test_first_enabled_and_job_history_survive_restart(self) -> None:
        with patch.object(status.time, "time", return_value=1000):
            status.update("enabled")
        with patch.object(status.time, "time", return_value=1100):
            status.update("start", "backup")
        with patch.object(status.time, "time", return_value=1200):
            status.update("success", "backup")
        with patch.object(status.time, "time", return_value=2000):
            status.update("enabled")

        state = status._read_state()
        self.assertEqual(state["first_enabled_timestamp_seconds"], 1000)
        self.assertEqual(state["startup_timestamp_seconds"], 2000)
        self.assertEqual(state["jobs"]["backup"]["started_timestamp_seconds"], 1100)
        self.assertEqual(state["jobs"]["backup"]["success_timestamp_seconds"], 1200)
        self.assertEqual(state["jobs"]["check"]["success_timestamp_seconds"], 0)

    def test_failure_is_recorded_as_last_result_without_losing_success(self) -> None:
        status.update("enabled")
        status.update("success", "check")
        previous_success = status._read_state()["jobs"]["check"]["success_timestamp_seconds"]
        status.update("failure", "check")

        metrics = status.METRICS_PATH.read_text(encoding="utf-8")
        self.assertIn('librepaper_backup_job_last_result_success{task="check"} 0', metrics)
        self.assertEqual(status._read_state()["jobs"]["check"]["success_timestamp_seconds"], previous_success)

    def test_metrics_are_readable_but_private_state_is_not(self) -> None:
        status.update("enabled")
        self.assertEqual(status.METRICS_PATH.stat().st_mode & 0o777, 0o644)
        self.assertEqual(status.STATE_PATH.stat().st_mode & 0o777, 0o600)
        text = status.METRICS_PATH.read_text(encoding="utf-8")
        self.assertIn('librepaper_backup_job_last_success_timestamp_seconds{task="backup"} 0', text)
        self.assertIn('librepaper_backup_job_last_success_timestamp_seconds{task="check"} 0', text)
        self.assertNotIn("repository", text)
        self.assertNotIn("password", text)

    def test_corrupt_state_exports_error_without_erasing_last_metrics(self) -> None:
        status.update("enabled")
        status.update("success", "backup")
        status.STATE_PATH.write_text("{broken", encoding="utf-8")
        previous_success = 'librepaper_backup_job_last_success_timestamp_seconds{task="backup"}'

        with patch.object(sys, "argv", [str(STATUS_FILE), "start", "check"]):
            result = status.main()

        self.assertEqual(result, 1)
        metrics = status.METRICS_PATH.read_text(encoding="utf-8")
        self.assertIn(previous_success, metrics)
        self.assertIn("librepaper_backup_state_error 1", metrics)

    def test_missing_state_after_metrics_exist_is_not_reinitialized(self) -> None:
        status.update("enabled")
        status.update("success", "backup")
        previous_success = status._read_state()["jobs"]["backup"]["success_timestamp_seconds"]
        status.STATE_PATH.unlink()

        with patch.object(sys, "argv", [str(STATUS_FILE), "enabled"]):
            result = status.main()

        self.assertEqual(result, 1)
        metrics = status.METRICS_PATH.read_text(encoding="utf-8")
        self.assertIn(f'librepaper_backup_job_last_success_timestamp_seconds{{task="backup"}} {previous_success}', metrics)
        self.assertIn("librepaper_backup_state_error 1", metrics)


class EntrypointTests(unittest.TestCase):
    def _run_entrypoint(self, config: str, *, show_status: int = 0) -> tuple[subprocess.CompletedProcess[str], Path]:
        temp = tempfile.TemporaryDirectory()
        root = Path(temp.name)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        status_log = root / "status.log"
        profile_log = root / "profile.log"
        config_path = root / "config.toml"
        profile_path = root / "profiles.toml"
        crontab = root / "crontab"
        config_path.write_text(config, encoding="utf-8")
        profile_path.write_text("", encoding="utf-8")
        status_command = bin_dir / "librepaper-backup-status"
        status_command.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{status_log}"\n', encoding="utf-8")
        status_command.chmod(0o755)
        profile_command = bin_dir / "resticprofile"
        profile_command.write_text(
            f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{profile_log}"\n'
            f'if [ "$5" = schedule ]; then printf "* * * * * true\\n" > "{crontab}"; fi\n'
            f'exit {show_status}\n',
            encoding="utf-8",
        )
        profile_command.chmod(0o755)
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{bin_dir}:{env['PATH']}",
                "LIBREPAPER_BACKUP_CONFIG": str(config_path),
                "LIBREPAPER_BACKUP_PROFILE": str(profile_path),
                "LIBREPAPER_BACKUP_CRONTAB": str(crontab),
                "LIBREPAPER_BACKUP_VALIDATE_ONLY": "1",
            }
        )
        result = subprocess.run(["sh", str(ENTRYPOINT)], env=env, text=True, capture_output=True, check=False)
        # Keep the temporary directory alive until the caller reads the traces.
        self.addCleanup(temp.cleanup)
        return result, root

    def test_validate_only_builds_schedule_without_starting_scheduler(self) -> None:
        result, root = self._run_entrypoint('[resticprofile]\nrepository = "secret://redact-me"\n')
        self.assertEqual(result.returncode, 0)
        self.assertTrue((root / "crontab").is_file())
        self.assertNotIn("redact-me", result.stdout + result.stderr)
        self.assertIn("-n resticprofile show", (root / "profile.log").read_text(encoding="utf-8"))
        self.assertIn("-n resticprofile schedule", (root / "profile.log").read_text(encoding="utf-8"))
        self.assertFalse((root / "status.log").exists())

    def test_invalid_toml_exits_without_echoing_config_values(self) -> None:
        result, root = self._run_entrypoint('token = "secret-value"\n[resticprofile\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("secret-value", result.stdout + result.stderr)
        self.assertEqual((root / "status.log").read_text(encoding="utf-8").strip(), "config-error")
        self.assertFalse((root / "profile.log").exists())

    def test_profile_validation_failure_is_recorded(self) -> None:
        result, root = self._run_entrypoint('[resticprofile]\nrepository = "secret-value"\n', show_status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("secret-value", result.stdout + result.stderr)
        self.assertEqual((root / "status.log").read_text(encoding="utf-8").strip().splitlines(), ["enabled", "config-error"])


if __name__ == "__main__":
    unittest.main()
