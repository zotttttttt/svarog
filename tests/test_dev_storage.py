"""Safety tests for the maintenance tool; run with python3 -m unittest discover -s tests -p 'test_*.py'."""

import argparse
import contextlib
import fcntl
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/dev-storage"
loader = importlib.machinery.SourceFileLoader("dev_storage", str(SCRIPT))
spec = importlib.util.spec_from_loader(loader.name, loader)
storage = importlib.util.module_from_spec(spec)
loader.exec_module(storage)


class StorageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name).resolve()
        storage.REPO = self.repo

    def run_command(self, script, keep=False):
        with contextlib.redirect_stdout(io.StringIO()):
            return storage.run_command(argparse.Namespace(
                command=[sys.executable, "-c", script], keep=keep))

    def cleanup(self, apply=False, builds=False):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            storage.cleanup(argparse.Namespace(apply=apply, build_artifacts=builds))
        return output.getvalue()

    def fixture_run(self, status="failed", age=8, keep=False):
        root = self.repo / ".svarog-runs"
        root.mkdir(exist_ok=True)
        (root / ".lock").touch()
        run = root / "run-fixture"
        run.mkdir()
        (run / ".lock").touch()
        (run / "output.log").write_text("failure details")
        storage.write_result(run, {"schema": 1, "status": status, "keep": keep,
                                  "started": time.time() - age * 86400})
        for item in [*run.iterdir(), run]:
            os.utime(item, (time.time() - age * 86400,) * 2)
        return run

    def old_build(self, path):
        path.mkdir(parents=True)
        (path / "artifact").write_text("rebuildable")
        for item in [path / "artifact", path]:
            os.utime(item, (time.time() - storage.WEEK - 100,) * 2)
        return path

    def test_success_removes_scratch_and_log(self):
        self.assertEqual(self.run_command(
            "import os,pathlib; pathlib.Path(os.environ['SVAROG_RUN_DIR'],'scratch').touch()"), 0)
        self.assertEqual(list((self.repo / ".svarog-runs").glob("run-*")), [])

    def test_failure_retains_output_and_diagnostic_files(self):
        code = self.run_command("import os,pathlib,sys; "
                                "pathlib.Path(os.environ['SVAROG_RUN_DIR'],'trace').write_text('trace'); "
                                "print('failure detail');sys.exit(7)")
        self.assertEqual(code, 7)
        run = next((self.repo / ".svarog-runs").glob("run-*"))
        self.assertIn("failure detail", (run / "output.log").read_text())
        self.assertEqual((run / "work/trace").read_text(), "trace")
        result = json.loads((run / "result.json").read_text())
        self.assertEqual(result["exit_code"], 7)
        self.assertEqual(result["status"], "failed")
        self.assertNotIn("DELETE", self.cleanup(apply=True))

    def test_keep_success_retains_full_run(self):
        self.assertEqual(self.run_command("print('ok')", keep=True), 0)
        run = next((self.repo / ".svarog-runs").glob("run-*"))
        self.assertTrue((run / "work").is_dir())
        self.cleanup(apply=True)
        self.assertTrue(run.exists())

    def test_new_validation_prunes_expired_runs(self):
        run = self.fixture_run()
        self.assertEqual(self.run_command("print('ok')"), 0)
        self.assertFalse(run.exists())

    def test_preview_then_delete_expired_failure(self):
        run = self.fixture_run()
        self.assertIn("WOULD DELETE", self.cleanup())
        self.assertTrue(run.exists())
        self.cleanup(apply=True)
        self.assertFalse(run.exists())

    def test_recent_diagnostics_extend_retention_after_abandoned_run(self):
        run = self.fixture_run(status="running")
        (run / "output.log").write_text("new failure detail")
        self.cleanup(apply=True)
        self.assertTrue(run.exists())

    def test_six_day_failure_is_not_expired(self):
        run = self.fixture_run(age=6)
        self.cleanup(apply=True)
        self.assertTrue(run.exists())

    def test_kept_expired_failure_survives(self):
        run = self.fixture_run(keep=True)
        self.cleanup(apply=True)
        self.assertTrue(run.exists())

    def test_locked_run_survives_even_if_metadata_is_expired(self):
        run = self.fixture_run(status="running")
        with storage.locked(run / ".lock"):
            self.assertIn("SKIP active", self.cleanup(apply=True))
        self.assertTrue(run.exists())
        self.cleanup(apply=True)
        self.assertFalse(run.exists())

    def test_live_child_retains_lock(self):
        run = self.fixture_run(status="running")
        with storage.locked(run / ".lock") as fd:
            child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"],
                                     pass_fds=(fd,))
        try:
            self.assertIn("SKIP active", self.cleanup(apply=True))
        finally:
            child.terminate()
            child.wait()
        self.assertTrue(run.exists())

    def test_symlink_inside_run_rejected(self):
        run = self.fixture_run()
        outside = self.repo / "source"
        outside.mkdir()
        (outside / "secret").write_text("secret")
        (run / "escape").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.cleanup(apply=True)
        self.assertEqual((outside / "secret").read_text(), "secret")

    def test_symlink_root_and_ancestor_rejected(self):
        outside = self.repo / "source"
        outside.mkdir()
        (self.repo / "target").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.cleanup(apply=True, builds=True)
        with self.assertRaises(ValueError):
            storage.checked(self.repo / "target/package")
        with self.assertRaises(ValueError):
            storage.checked(self.repo.parent / "escape")
        with self.assertRaises(ValueError):
            storage.checked(self.repo / ".." / "escape")

    def test_symlink_lock_rejected(self):
        run = self.fixture_run()
        source = self.repo / "secret"
        source.write_text("keep")
        (run / ".lock").unlink()
        (run / ".lock").symlink_to(source)
        with self.assertRaises(ValueError):
            self.cleanup(apply=True)
        self.assertEqual(source.read_text(), "keep")

    def test_recent_build_and_native_caches_preserved(self):
        target = self.repo / "target"
        for profile in ("debug", "release"):
            path = self.old_build(target / profile)
            (path / ".cargo-lock").touch()
        package = self.old_build(target / "package")
        (package / "recent").touch()
        self.assertIn("SKIP recent", self.cleanup(apply=True, builds=True))
        self.assertTrue(package.exists())
        self.assertTrue((target / "debug/artifact").exists())
        self.assertTrue((target / "release/artifact").exists())

    def test_build_cleanup_is_opt_in_and_skips_cargo_lock(self):
        target = self.repo / "target"
        (target / "debug").mkdir(parents=True)
        lock = target / "debug/.cargo-lock"
        lock.touch()
        package = self.old_build(target / "package")
        self.cleanup(apply=True)
        self.assertTrue(package.exists())
        with storage.locked(lock):
            self.assertIn("SKIP active", self.cleanup(apply=True, builds=True))
        self.assertTrue(package.exists())
        self.cleanup(apply=True, builds=True)
        self.assertFalse(package.exists())

    def test_missing_cargo_lock_refuses_deletion(self):
        (self.repo / "target/debug").mkdir(parents=True)
        package = self.old_build(self.repo / "target/package")
        with self.assertRaises(ValueError):
            self.cleanup(apply=True, builds=True)
        self.assertTrue(package.exists())

    def test_read_only_report_creates_nothing(self):
        before = list(self.repo.rglob("*"))
        with contextlib.redirect_stdout(io.StringIO()):
            storage.report()
        self.assertEqual(before, list(self.repo.rglob("*")))

    def test_usage_does_not_double_count_hardlinked_binaries(self):
        target = self.repo / "target"
        target.mkdir()
        binary = target / "binary"
        binary.write_bytes(b"a" * 8192)
        size = storage.usage(target)[0]
        os.link(binary, target / "linked-binary")
        self.assertEqual(storage.usage(target)[0], size)

    def test_cross_release_cleanup_preserves_sources_and_state(self):
        target = self.repo / "target"
        cross = target / "aarch64-apple-darwin"
        release = self.old_build(cross / "release")
        (cross / "CACHEDIR.TAG").write_text("cache")
        lock = release / ".cargo-lock"
        lock.touch()
        os.utime(lock, (time.time() - storage.WEEK - 100,) * 2)
        os.utime(release, (time.time() - storage.WEEK - 100,) * 2)
        source = self.repo / "source.rs"
        secret = self.repo / ".env"
        source.write_text("source")
        secret.write_text("secret")
        (self.repo / ".svarog-dev").mkdir()
        (self.repo / ".svarog-dev/state").write_text("state")
        with storage.locked(lock):
            self.assertIn("SKIP active", self.cleanup(apply=True, builds=True))
        self.cleanup(apply=True, builds=True)
        self.assertFalse(release.exists())
        self.assertEqual(source.read_text(), "source")
        self.assertEqual(secret.read_text(), "secret")
        self.assertEqual((self.repo / ".svarog-dev/state").read_text(), "state")

    def test_build_symlink_refused_before_any_deletions(self):
        package = self.old_build(self.repo / "target/package")
        flycheck = self.old_build(self.repo / "target/flycheck0")
        secret = self.repo / ".env"
        secret.write_text("secret")
        (flycheck / "escape").symlink_to(secret)
        with self.assertRaises(ValueError):
            self.cleanup(apply=True, builds=True)
        self.assertTrue(package.exists())
        self.assertEqual(secret.read_text(), "secret")

    def test_metadata_rejected_before_deletion(self):
        run = self.fixture_run()
        (run / "result.json").write_text('{"schema": 200}')
        with self.assertRaises(ValueError):
            self.cleanup(apply=True)
        self.assertTrue(run.exists())

    def test_execution_failure_retains_diagnostics(self):
        with contextlib.redirect_stdout(io.StringIO()):
            code = storage.run_command(argparse.Namespace(command=["/no/such/tool"], keep=False))
        self.assertEqual(code, 127)
        run = next((self.repo / ".svarog-runs").glob("run-*"))
        self.assertIn("Could not execute", (run / "output.log").read_text())

    def test_interrupted_run_retains_diagnostics_and_returns_signal_status(self):
        script = self.repo / "scripts/dev-storage"
        script.parent.mkdir()
        shutil.copyfile(SCRIPT, script)
        process = subprocess.Popen(
            [sys.executable, str(script), "run", "--", sys.executable, "-u", "-c",
             "import time;print('ready');time.sleep(30)"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                logs = list((self.repo / ".svarog-runs").glob("run-*/output.log"))
                if logs and "ready" in logs[0].read_text():
                    break
                time.sleep(0.01)
            else:
                self.fail("validation child never started")
            process.terminate()
            stdout, stderr = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 143, (stdout, stderr))
            result = json.loads((logs[0].parent / "result.json").read_text())
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["exit_code"], -15)
            self.assertIn("ready", logs[0].read_text())
        finally:
            if process.poll() is None:
                process.terminate()
                process.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
