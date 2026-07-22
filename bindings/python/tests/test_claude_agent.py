"""kriya.agents.claude_agent against the REAL claude-agent-sdk + kriya-govern binary.

Skips if either claude-agent-sdk (`pip install claude-agent-sdk`) or the binary isn't present.
"""

import asyncio
import json
import os
import tempfile
import unittest
from pathlib import Path

from kriya.agents import GovernClient

_HERE = Path(__file__).resolve().parent

try:
    from claude_agent_sdk import tool

    from kriya.agents.claude_agent import govern_sdk_tool

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


@unittest.skipUnless(BIN and _HAVE_SDK, "needs kriya-govern + `pip install claude-agent-sdk`")
class TestClaudeAgent(unittest.TestCase):
    def _client(self, policy_yaml):
        d = tempfile.mkdtemp(prefix="kriya-claude-py-")
        policy = os.path.join(d, "policy.yaml")
        Path(policy).write_text(policy_yaml)
        log = os.path.join(d, "audit.jsonl")
        return GovernClient(binary_path=BIN, policy_path=policy, actor="claude-agent", user="ci", audit_log=log), log

    def test_allowed_tool_runs_and_signs_a_receipt(self):
        client, log = self._client(
            'rules:\n  - { action: "search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        @tool("search", "Search the web", {"query": str})
        async def search(args):
            return {"content": [{"type": "text", "text": f"results for {args['query']}"}]}

        self.assertIs(govern_sdk_tool(client, search), search)
        self.assertEqual(search.name, "search")

        result = asyncio.run(search.handler({"query": "kriya"}))
        self.assertFalse(result.get("isError"))
        self.assertEqual(result["content"][0]["text"], "results for kriya")

        receipt = json.loads(Path(log).read_text().strip())
        self.assertEqual(receipt["action_id"], "search")
        self.assertTrue(receipt["success"])
        self.assertEqual(receipt["actor"]["agent"], "claude-agent")
        client.close()

    def test_denied_tool_returns_iserror_and_signs_no_receipt(self):
        client, log = self._client('rules:\n  - { action: "*", allow: false }\n')

        @tool("wipe_db", "danger", {"target": str})
        async def wipe_db(args):
            return {"content": [{"type": "text", "text": "wiped"}]}

        govern_sdk_tool(client, wipe_db)
        result = asyncio.run(wipe_db.handler({"target": "/"}))
        self.assertTrue(result.get("isError"))
        self.assertIn("denied", json.dumps(result).lower())
        self.assertTrue(not Path(log).exists() or Path(log).read_text().strip() == "")
        client.close()

    def test_handler_iserror_records_success_false(self):
        client, log = self._client(
            'rules:\n  - { action: "search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        @tool("search", "Search the web", {"query": str})
        async def failing(args):
            return {"content": [{"type": "text", "text": "upstream error"}], "isError": True}

        govern_sdk_tool(client, failing)
        result = asyncio.run(failing.handler({"query": "x"}))
        self.assertTrue(result.get("isError"))  # the handler's own error result is preserved
        receipt = json.loads(Path(log).read_text().strip())
        self.assertFalse(receipt["success"])  # recorded as a failed call
        client.close()


if __name__ == "__main__":
    unittest.main()
