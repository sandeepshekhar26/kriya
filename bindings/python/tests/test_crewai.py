"""kriya.agents.crewai against the REAL crewai package + kriya-govern binary.

Skips if either crewai (`pip install crewai`) or the binary isn't present. `govern()` already covers
this framework generically; this drives a REAL CrewAI Tool through the adapter to prove it end to end.
"""

import json
import os
import tempfile
import unittest
from pathlib import Path

# Keep the test hermetic/offline — disable CrewAI's telemetry before importing it.
os.environ.setdefault("CREWAI_DISABLE_TELEMETRY", "true")
os.environ.setdefault("OTEL_SDK_DISABLED", "true")

from kriya.agents import GovernClient, GovernDenied

_HERE = Path(__file__).resolve().parent

try:
    from crewai.tools import tool as crew_tool

    from kriya.agents.crewai import govern_crew_tool

    _HAVE_CREWAI = True
except Exception:
    _HAVE_CREWAI = False


def _find_bin():
    env = os.environ.get("KRIYA_GOVERN_BIN")
    if env and Path(env).exists():
        return env
    for rel in (
        "../../../apps/note-app/src-tauri/target/debug/kriya-govern",
        "../../../crates/kriya/target/debug/kriya-govern",
        "../../../target/debug/kriya-govern",
    ):
        p = (_HERE / rel).resolve()
        if p.exists():
            return str(p)
    return None


BIN = _find_bin()


@unittest.skipUnless(BIN and _HAVE_CREWAI, "needs kriya-govern + `pip install crewai`")
class TestCrewAI(unittest.TestCase):
    def _client(self, policy_yaml):
        d = tempfile.mkdtemp(prefix="kriya-crew-")
        policy = os.path.join(d, "policy.yaml")
        Path(policy).write_text(policy_yaml)
        log = os.path.join(d, "audit.jsonl")
        return GovernClient(binary_path=BIN, policy_path=policy, actor="crewai", user="ci", audit_log=log), log

    def test_allowed_tool_runs_via_tool_run_and_signs_a_receipt(self):
        client, log = self._client(
            'rules:\n  - { action: "add", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        @crew_tool("add")
        def add(a: int, b: int) -> str:
            "Add two numbers."
            return str(a + b)

        self.assertIs(govern_crew_tool(client, add), add)
        self.assertEqual(add.name, "add")

        # Drive it the way a CrewAI agent does.
        out = add.run(a=2, b=3)
        self.assertEqual(out, "5")

        receipt = json.loads(Path(log).read_text().strip())
        self.assertEqual(receipt["action_id"], "add")
        self.assertTrue(receipt["success"])
        self.assertEqual(receipt["params"], {"a": 2, "b": 3})
        self.assertEqual(receipt["actor"]["agent"], "crewai")
        client.close()

    def test_denied_tool_raises_governdenied_and_signs_no_receipt(self):
        client, log = self._client('rules:\n  - { action: "*", allow: false }\n')

        @crew_tool("wipe_db")
        def wipe_db(target: str) -> str:
            "danger"
            return f"wiped {target}"

        govern_crew_tool(client, wipe_db)
        with self.assertRaises(GovernDenied):
            wipe_db.run(target="/")
        self.assertTrue(not Path(log).exists() or Path(log).read_text().strip() == "")
        client.close()


if __name__ == "__main__":
    unittest.main()
