"""run_task wiring -- driven by a scripted in-memory host (no real binary).

The MockHost reuses the real Host's dispatch/emit machinery and replays a scripted sequence of
host->app messages, advancing one step each time the app sends a reply. This exercises run_task's
action/approval/step/done wiring and the registry-driven dispatch path end to end, synchronously.
"""

import io
import json
import unittest

from kriya import Host, Registry, ok, required, run_task, string


class MockHost(Host):
    def __init__(self, turns):
        super().__init__(stdin=io.StringIO())
        self._turns = turns  # list of (type, data) host->app messages, in order
        self._i = 0
        self.sent = []  # (type, data) app->host messages captured

    def _fire_next(self):
        if self._i < len(self._turns):
            t, d = self._turns[self._i]
            self._i += 1
            self._dispatch_line(json.dumps({"type": t, "data": d}))

    # Override sends: capture, then advance the script (drives the next host->app message).
    def start(self, request):
        self.sent.append(("start", request.to_wire()))
        self._fire_next()

    def send_action_result(self, result):
        self.sent.append(("action_result", result.to_wire()))
        self._fire_next()

    def send_approval(self, response):
        self.sent.append(("approval_response", response.to_wire()))
        self._fire_next()

    def send_step_advance(self, advance):
        self.sent.append(("step_advance", advance.to_wire()))
        self._fire_next()


class TestRunTask(unittest.TestCase):
    def _registry(self):
        reg = Registry()
        notes = []

        def create(p, ctx):
            notes.append(p["title"])
            return ok({"id": len(notes)})

        reg.register_action(
            id="create_note",
            description="Create a note.",
            parameters={"title": required(string)},
            handler=create,
        )
        reg.register_action(
            id="delete_note",
            description="Delete a note.",
            parameters={"id": required(string)},
            handler=lambda p, ctx: ok({"deleted": p["id"]}),
        )
        return reg, notes

    def test_registry_driven_run(self):
        reg, notes = self._registry()
        host = MockHost(
            [
                ("action", {"stepId": "s1", "actionId": "create_note", "params": {"title": "Groceries"}, "reasoning": "seed"}),
                ("done", {"summary": "made a note", "steps": 1}),
            ]
        )
        state = {"notes": notes}
        done = run_task(host, goal="tidy", state=lambda: {"notes": list(notes)}, registry=reg)
        self.assertEqual(done.steps, 1)
        self.assertEqual(done.summary, "made a note")
        self.assertEqual(notes, ["Groceries"])

        # start carried the tool menu derived from the registry.
        start = dict(host.sent)["start"]
        tool_names = {t["name"] for t in start["tools"]}
        self.assertEqual(tool_names, {"create_note", "delete_note"})

        # the action_result carried success + refreshed state.
        ar = [d for (t, d) in host.sent if t == "action_result"][0]
        self.assertTrue(ar["success"])
        self.assertEqual(ar["state"], {"notes": ["Groceries"]})

    def test_approval_gate(self):
        reg, _ = self._registry()
        host = MockHost(
            [
                ("approval", {"stepId": "s1", "actionId": "delete_note", "params": {"id": "1"}, "reasoning": "cleanup"}),
                ("action", {"stepId": "s1", "actionId": "delete_note", "params": {"id": "1"}, "reasoning": "cleanup"}),
                ("done", {"summary": "done", "steps": 1}),
            ]
        )
        asked = []
        done = run_task(
            host,
            goal="cleanup",
            state={},
            registry=reg,
            approve=lambda req: asked.append(req.action_id) or True,
        )
        self.assertEqual(asked, ["delete_note"])
        appr = [d for (t, d) in host.sent if t == "approval_response"][0]
        self.assertEqual(appr, {"stepId": "s1", "approved": True})
        self.assertEqual(done.steps, 1)

    def test_default_approval_denies(self):
        reg, _ = self._registry()
        host = MockHost(
            [
                ("approval", {"stepId": "s1", "actionId": "delete_note", "params": {"id": "1"}, "reasoning": "x"}),
                ("done", {"summary": "done", "steps": 0}),
            ]
        )
        run_task(host, goal="g", state={}, registry=reg)  # no approve= -> deny
        appr = [d for (t, d) in host.sent if t == "approval_response"][0]
        self.assertFalse(appr["approved"])

    def test_step_mode_default_advances(self):
        reg, notes = self._registry()
        host = MockHost(
            [
                ("await_step", {"gateId": "g1", "stepNumber": 1}),
                ("action", {"stepId": "s1", "actionId": "create_note", "params": {"title": "X"}, "reasoning": "r"}),
                ("done", {"summary": "done", "steps": 1}),
            ]
        )
        run_task(host, goal="g", state={}, registry=reg, step_mode=True)
        adv = [d for (t, d) in host.sent if t == "step_advance"][0]
        self.assertEqual(adv, {"gateId": "g1", "proceed": True})

    def test_manual_dispatch_path(self):
        from kriya import ActionResultMsg

        host = MockHost(
            [
                ("action", {"stepId": "s9", "actionId": "anything", "params": {}, "reasoning": "r"}),
                ("done", {"summary": "ok", "steps": 1}),
            ]
        )
        seen = []

        def dispatch(req):
            seen.append(req.action_id)
            return ActionResultMsg(step_id=req.step_id, success=True, state={"ran": req.action_id})

        done = run_task(host, goal="g", state={}, dispatch=dispatch)
        self.assertEqual(seen, ["anything"])
        self.assertEqual(done.summary, "ok")

    def test_requires_registry_or_dispatch(self):
        host = MockHost([])
        with self.assertRaises(ValueError):
            run_task(host, goal="g", state={})


if __name__ == "__main__":
    unittest.main()
