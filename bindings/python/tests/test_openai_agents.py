"""kriya.agents.openai_agents against the REAL @openai/agents (Python) SDK + kriya-govern binary.

Skips if either the SDK (`pip install openai-agents`) or the binary isn't present, so a bare test run
doesn't fail. `govern()` already covers this framework generically; this drives a REAL FunctionTool's
invocation seam through the adapter to prove it end to end.
"""

import asyncio
import json
import os
import tempfile
import unittest
from pathlib import Path

from kriya.agents import GovernClient, GovernDenied

_HERE = Path(__file__).resolve().parent

try:
    from agents import function_tool
    from agents.tool_context import ToolContext

    from kriya.agents.openai_agents import govern_function_tool

    _HAVE_SDK = True
except Exception:
    _HAVE_SDK = False


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


def _tool_ctx(name, args_json):
    # A minimal ToolContext the SDK's on_invoke_tool accepts (no agent run required).
    return ToolContext(context=None, tool_name=name, tool_call_id="test-call", tool_arguments=args_json)


@unittest.skipUnless(BIN and _HAVE_SDK, "needs kriya-govern + `pip install openai-agents`")
class TestOpenAIAgents(unittest.TestCase):
    def _client(self, policy_yaml):
        d = tempfile.mkdtemp(prefix="kriya-oai-py-")
        policy = os.path.join(d, "policy.yaml")
        Path(policy).write_text(policy_yaml)
        log = os.path.join(d, "audit.jsonl")
        return GovernClient(binary_path=BIN, policy_path=policy, actor="openai-agents", user="ci", audit_log=log), log

    def test_allowed_tool_runs_through_the_sdk_seam_and_signs_a_receipt(self):
        client, log = self._client(
            'rules:\n  - { action: "add", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        @function_tool
        def add(a: int, b: int) -> str:
            "Add two numbers."
            return str(a + b)

        self.assertIs(govern_function_tool(client, add), add)  # same object, name/schema preserved
        self.assertEqual(add.name, "add")

        args = json.dumps({"a": 2, "b": 3})
        out = asyncio.run(add.on_invoke_tool(_tool_ctx("add", args), args))
        self.assertEqual(out, "5")

        receipt = json.loads(Path(log).read_text().strip())
        self.assertEqual(receipt["action_id"], "add")
        self.assertTrue(receipt["success"])
        self.assertEqual(receipt["actor"]["agent"], "openai-agents")
        client.close()

    def test_denied_tool_raises_governdenied_and_signs_no_receipt(self):
        client, log = self._client('rules:\n  - { action: "*", allow: false }\n')

        @function_tool
        def wipe_db() -> str:
            "danger"
            return "wiped"

        govern_function_tool(client, wipe_db)
        with self.assertRaises(GovernDenied):
            asyncio.run(wipe_db.on_invoke_tool(_tool_ctx("wipe_db", "{}"), "{}"))
        self.assertTrue(not Path(log).exists() or Path(log).read_text().strip() == "")
        client.close()


if __name__ == "__main__":
    unittest.main()
