import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("drain", Path(__file__).with_name("rumi_drained_upgrade.py"))
drain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drain)


class Result:
    def __init__(self, stdout="", code=0):
        self.stdout, self.stderr, self.returncode = stdout, "", code


class Fake:
    def __init__(self, test, mode="ok"):
        self.test, self.mode, self.actions = test, mode, []
        self.state, self.module = "Running", test.old
    def __call__(self, argv, timeout):
        if argv == ["icp", "--version"]:
            return Result("icp 1.3.0\n")
        verb = argv[2]
        self.actions.append(verb)
        if verb == "status":
            if self.mode == "public":
                return Result(json.dumps({"id": "target", "module_hash": self.module, "controllers": []}))
            return Result(json.dumps({"id": "target", "status": self.state, "module_hash": self.module, "settings": {}}))
        if verb == self.mode + "_timeout":
            raise drain.Hold("timeout")
        if self.mode == verb + "_timeout":
            raise drain.Hold("timeout")
        if self.mode == verb + "_error":
            return Result(code=1)
        if verb == "stop":
            self.state = "Stopped" if self.mode != "still_stopping" else "Stopping"
        elif verb == "install":
            self.test.assertIn("upgrade", argv)
            self.test.assertNotIn("reinstall", argv)
            self.test.assertEqual(Path(argv[argv.index("--wasm") + 1]).read_bytes(), b"new wasm")
            self.test.assertEqual(argv[argv.index("--args") + 1], "(variant { Upgrade })")
            self.module = self.test.new
        elif verb == "start":
            self.state = "Running"
        return Result()


class DrainTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        (base / "wasm").write_bytes(b"new wasm")
        (base / "args").write_bytes(b"(variant { Upgrade })")
        self.old = "1" * 64
        self.new = drain.sha(b"new wasm")
        self.args = SimpleNamespace(execute=True, target="target", environment="mainnet-live", identity="operator",
                                    wasm=str(base / "wasm"), args_file=str(base / "args"),
                                    current_sha256=self.old, new_sha256=self.new,
                                    args_sha256=drain.sha(b"(variant { Upgrade })"),
                                    journal=str(base / "journal"), timeout=10)
    def test_success_requires_stopped_then_verified_new_module_before_start(self):
        fake = Fake(self)
        drain.execute(self.args, fake)
        self.assertEqual(fake.actions, ["status", "stop", "status", "install", "status", "start", "status"])
        events = [json.loads(x)["event"] for x in Path(self.args.journal).read_text().splitlines()]
        self.assertEqual(events[-1], "running_verified")
        with self.assertRaises(FileExistsError):
            drain.execute(self.args, Fake(self))
    def test_public_fallback_fails_before_stop(self):
        fake = Fake(self, "public")
        with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
        self.assertEqual(fake.actions, ["status"])
    def test_unknown_stop_never_installs_or_restarts(self):
        for mode in ("stop_timeout", "stop_error", "still_stopping"):
            with self.subTest(mode=mode):
                self.args.journal = str(Path(self.tmp.name) / mode)
                fake = Fake(self, mode)
                with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
                self.assertNotIn("install", fake.actions)
                self.assertNotIn("start", fake.actions)
    def test_unknown_upgrade_is_never_retried_and_never_started(self):
        for mode in ("install_timeout", "install_error"):
            with self.subTest(mode=mode):
                self.args.journal = str(Path(self.tmp.name) / mode)
                fake = Fake(self, mode)
                with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
                self.assertEqual(fake.actions.count("install"), 1)
                self.assertNotIn("start", fake.actions)
    def test_unknown_start_is_not_retried(self):
        fake = Fake(self, "start_timeout")
        with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
        self.assertEqual(fake.actions.count("start"), 1)
    def test_local_target_lock_blocks_other_environment_before_cli(self):
        fake = Fake(self)
        with drain.target_lock(self.args.target):
            with self.assertRaises(drain.Hold):
                drain.execute(self.args, fake)
        self.assertEqual(fake.actions, [])

    def test_bad_artifact_and_absent_execute_never_call_cli(self):
        fake = Fake(self)
        self.args.execute = False
        with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
        self.assertEqual(fake.actions, [])
        self.args.execute = True
        self.args.new_sha256 = "2" * 64
        with self.assertRaises(drain.Hold): drain.execute(self.args, fake)
        self.assertEqual(fake.actions, [])

if __name__ == "__main__":
    unittest.main()
