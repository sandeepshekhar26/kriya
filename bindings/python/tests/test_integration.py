"""End-to-end integration test against the REAL kriya-host binary.

Opt-in: set ``KRIYA_HOST_BIN`` to the built ``kriya-host`` path, or build it at the default
location::

    (cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)

Skipped (not failed) when the binary isn't present, so the pure-Python suite stays self-contained
and CI without a Rust toolchain still passes. This is the Python mirror of the kriya-sidecar
integration test: it drives a scripted run through action dispatch + a held/granted approval +
durable memory recall, all against the real governed host.
"""

import json
import os
import tempfile
import time
import unittest
from pathlib import Path

from kriya import Host, Registry, ok, required, run_task, string


def _host_binary():
    env = os.environ.get("KRIYA_HOST_BIN")
    if env:
        return env if Path(env).exists() else None
    default = (
        Path(__file__).resolve().parents[3]
        / "apps"
        / "note-app"
        / "src-tauri"
        / "target"
        / "debug"
        / "kriya-host"
    )
    return str(default) if default.exists() else None


_BIN = _host_binary()


@unittest.skipUnless(
    _BIN,
    "kriya-host not built; set KRIYA_HOST_BIN or build it (see module docstring)",
)
class TestRealHostIntegration(unittest.TestCase):
    def _registry(self, notes):
        reg = Registry()
        reg.register_action(
            id="create_note",
            description="Create a note.",
            parameters={"title": required(string)},
            handler=lambda p, ctx: (notes.append(p["title"]), ok({"id": len(notes)}))[1],
        )
        reg.register_action(
            id="delete_note",
            description="Delete a note.",
            parameters={"id": required(string)},
            handler=lambda p, ctx: ok({"deleted": p["id"]}),
        )
        return reg

    def test_scripted_run_end_to_end(self):
        with tempfile.TemporaryDirectory() as d:
            script = Path(d) / "script.json"
            script.write_text(
                json.dumps(
                    [
                        {"action": "create_note", "params": {"title": "From Python"}, "reasoning": "seed"},
                        {"done": True, "summary": "done"},
                    ]
                )
            )
            notes = []
            host = Host.spawn(_BIN, ["--script", str(script)])
            try:
                done = run_task(
                    host,
                    goal="py-itest-basic",
                    state=lambda: {"notes": list(notes)},
                    registry=self._registry(notes),
                    timeout=30.0,
                )
                self.assertEqual(done.steps, 1)
                self.assertEqual(notes, ["From Python"])
            finally:
                host.close()

    def test_held_approval_then_memory_recall(self):
        with tempfile.TemporaryDirectory() as d:
            script = Path(d) / "script.json"
            script.write_text(
                json.dumps(
                    [
                        {"action": "create_note", "params": {"title": "keep"}, "reasoning": "seed"},
                        {"action": "delete_note", "params": {"id": "1"}, "reasoning": "cleanup"},
                        {"done": True, "summary": "done"},
                    ]
                )
            )
            goal = f"py-itest-{time.time_ns()}"
            notes = []
            asked = []
            host = Host.spawn(_BIN, ["--script", str(script)])
            try:
                done = run_task(
                    host,
                    goal=goal,
                    state=lambda: {"notes": list(notes)},
                    registry=self._registry(notes),
                    approve=lambda req: (asked.append(req.action_id), True)[1],
                    timeout=30.0,
                )
                # delete_note is the only policy-guarded action (default policy: delete_* needs a human).
                self.assertEqual(asked, ["delete_note"])
                self.assertEqual(done.steps, 2)

                # Durable memory recall over the sidecar protocol -- both actions were signed + recorded.
                mine = [e.action_id for e in host.recent_memory(50) if e.goal == goal]
                self.assertIn("create_note", mine)
                self.assertIn("delete_note", mine)
            finally:
                host.close()


if __name__ == "__main__":
    unittest.main()
