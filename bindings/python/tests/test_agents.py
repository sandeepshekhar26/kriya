"""kriya.agents against the REAL kriya-govern binary.

The whole point of the middleware is that it signs NOTHING itself, so the acceptance runs against the
real binary. Without it built (a bare test run on a fresh checkout) the suite skips with a clear
reason rather than passing vacuously. Build it with:

    ( cd apps/note-app/src-tauri && cargo build -p kriya --locked --bin kriya-govern )
"""

import json
import os
import tempfile
import unittest
from pathlib import Path

from kriya.agents import GovernClient, GovernDenied, govern
from kriya.agents.langgraph import govern_tool

_HERE = Path(__file__).resolve().parent


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


def _policy(yaml: str) -> str:
    d = tempfile.mkdtemp(prefix="kriya-agents-py-")
    p = os.path.join(d, "policy.yaml")
    Path(p).write_text(yaml)
    return p


@unittest.skipUnless(BIN, "kriya-govern binary not built — see the module docstring")
class TestGovernClient(unittest.TestCase):
    def setUp(self):
        self._clients = []

    def tearDown(self):
        for c in self._clients:
            c.close()

    def _make(self, policy_yaml, approval="deny"):
        d = tempfile.mkdtemp(prefix="kriya-agents-py-log-")
        audit_log = os.path.join(d, "audit.jsonl")
        c = GovernClient(
            binary_path=BIN,
            policy_path=_policy(policy_yaml),
            actor="langgraph",
            user="ci",
            approval=approval,
            audit_log=audit_log,
        )
        self._clients.append(c)
        return c, audit_log

    def test_allow_runs_and_signs_a_verifiable_receipt(self):
        client, log = self._make(
            'rules:\n  - { action: "web_search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )
        ran = {"n": 0}

        @govern(client, "web_search")
        def web_search(params):
            ran["n"] += 1
            return f"results for {params['q']}"

        self.assertEqual(web_search({"q": "kriya"}), "results for kriya")
        self.assertEqual(ran["n"], 1)

        receipt = json.loads(Path(log).read_text().strip().splitlines()[0])
        self.assertEqual(receipt["action_id"], "web_search")
        self.assertTrue(receipt["success"])
        self.assertEqual(receipt["actor"], {"agent": "langgraph", "user": "ci"})
        # A real Ed25519 signature + public key from the runtime Signer -- not this package.
        self.assertRegex(receipt["signature"], r"^[0-9a-f]{128}$")
        self.assertRegex(receipt["public_key"], r"^[0-9a-f]{64}$")

    def test_deny_raises_and_never_runs_or_signs(self):
        client, log = self._make('rules:\n  - { action: "*", allow: false }\n')
        ran = {"n": 0}

        @govern(client, "delete_account")
        def delete_account(params):
            ran["n"] += 1
            return "gone"

        with self.assertRaises(GovernDenied):
            delete_account({})
        self.assertEqual(ran["n"], 0)
        self.assertTrue(not Path(log).exists() or Path(log).read_text().strip() == "")

    def test_failure_records_success_false_then_reraises(self):
        client, log = self._make(
            'rules:\n  - { action: "flaky", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        @govern(client, "flaky")
        def flaky(params):
            raise ValueError("upstream 500")

        with self.assertRaises(ValueError):
            flaky({})
        receipt = json.loads(Path(log).read_text().strip())
        self.assertEqual(receipt["action_id"], "flaky")
        self.assertFalse(receipt["success"])  # the attempt is recorded even though it failed

    def test_budget_cap_denies_further_calls(self):
        client, _ = self._make(
            'rules:\n  - { action: "tick", allow: true }\nbudget:\n  max_actions_per_minute: 2\n'
        )
        tick = govern(client, "tick", lambda p: "ok")
        self.assertEqual(tick({}), "ok")
        self.assertEqual(tick({}), "ok")
        with self.assertRaises(GovernDenied) as ctx:
            tick({})
        self.assertEqual(ctx.exception.decision, "budget_exceeded")

    def test_require_approval_denies_headless_and_auto_approves_when_told(self):
        deny_client, _ = self._make(
            'rules:\n  - { action: "send_email", allow: true, require_approval: true }\n'
        )
        send = govern(deny_client, "send_email", lambda p: "sent")
        with self.assertRaises(GovernDenied) as ctx:
            send({})
        self.assertEqual(ctx.exception.decision, "not_approved")

        auto_client, _ = self._make(
            'rules:\n  - { action: "send_email", allow: true, require_approval: true }\nbudget:\n  max_actions_per_minute: 60\n',
            approval="auto",
        )
        send2 = govern(auto_client, "send_email", lambda p: "sent")
        self.assertEqual(send2({}), "sent")

    def test_stamps_run_correlation_run_id_and_nested_parent_step_id(self):
        client, log = self._make(
            'rules:\n  - { action: "*", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
            approval="auto",
        )
        # A top-level call: the client's run_id groups the invocation; no parent.
        outer = govern(client, "outer", lambda p: "ok")
        outer({"q": "x"})
        outer_receipt = json.loads(Path(log).read_text().strip().splitlines()[0])
        # A nested call under the outer step's id.
        inner = govern(client, "inner", lambda p: "ok", parent_step_id=outer_receipt["step_id"])
        inner({})

        lines = Path(log).read_text().strip().splitlines()
        outer_r = json.loads(lines[0])
        inner_r = json.loads(lines[1])
        # Both share the client's run_id — the whole invocation is one run.
        self.assertEqual(outer_r["params"]["kriya.corr"]["run_id"], client.run_id)
        self.assertEqual(inner_r["params"]["kriya.corr"]["run_id"], client.run_id)
        # The top-level call has no parent; the nested call points at the outer step.
        self.assertNotIn("parent_step_id", outer_r["params"]["kriya.corr"])
        self.assertEqual(inner_r["params"]["kriya.corr"]["parent_step_id"], outer_r["step_id"])
        # The tool's own params are preserved alongside the reserved key.
        self.assertEqual(outer_r["params"]["q"], "x")

    def test_default_run_ids_are_distinct_and_a_supplied_run_id_is_honored(self):
        a = GovernClient(binary_path=BIN, run_id="run-external-42")
        b = GovernClient(binary_path=BIN)
        self._clients += [a, b]
        self.assertEqual(a.run_id, "run-external-42")
        self.assertNotEqual(b.run_id, a.run_id)  # a fresh UUID per client by default

    def test_langgraph_adapter_governs_a_keyword_called_tool(self):
        client, log = self._make(
            'rules:\n  - { action: "adder", allow: true }\nbudget:\n  max_actions_per_minute: 60\n'
        )

        # LangChain calls tool functions with keyword args: adder(a=2, b=3).
        governed = govern_tool(client, "adder", lambda a, b: a + b)
        self.assertEqual(governed(a=2, b=3), 5)
        receipt = json.loads(Path(log).read_text().strip())
        self.assertEqual(receipt["action_id"], "adder")
        # The tool's own params are preserved; run correlation (S3) rides the reserved key alongside.
        self.assertEqual(receipt["params"]["a"], 2)
        self.assertEqual(receipt["params"]["b"], 3)
        self.assertEqual(receipt["params"]["kriya.corr"]["run_id"], client.run_id)
        self.assertTrue(receipt["success"])


if __name__ == "__main__":
    unittest.main()
