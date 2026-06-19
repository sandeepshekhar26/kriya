"""Host wire framing, dispatch, send format, and memory correlation -- no real binary."""

import json
import threading
import time
import unittest

from kriya import ActionResultMsg, ApprovalResponse, Host, StepAdvance


class _Capture:
    """A stand-in for the host's stdin that records everything written."""

    def __init__(self):
        self.lines = []

    def write(self, s):
        self.lines.append(s)

    def flush(self):
        pass

    def close(self):
        pass

    def messages(self):
        text = "".join(self.lines)
        return [json.loads(line) for line in text.split("\n") if line.strip()]


def line(type_, data):
    return json.dumps({"type": type_, "data": data})


class TestHostFraming(unittest.TestCase):
    def test_action_line_becomes_typed_event(self):
        host = Host(stdin=_Capture())
        seen = []
        host.on("action", seen.append)
        host._dispatch_line(
            line(
                "action",
                {"stepId": "s1", "actionId": "create_note", "params": {"t": 1}, "reasoning": "r"},
            )
        )
        self.assertEqual(len(seen), 1)
        self.assertEqual(seen[0].action_id, "create_note")
        self.assertEqual(seen[0].step_id, "s1")
        self.assertEqual(seen[0].params, {"t": 1})

    def test_done_event(self):
        host = Host(stdin=_Capture())
        got = []
        host.on("done", got.append)
        host._dispatch_line(line("done", {"summary": "all done", "steps": 2}))
        self.assertEqual(got[0].summary, "all done")
        self.assertEqual(got[0].steps, 2)

    def test_await_step_camel_to_snake(self):
        host = Host(stdin=_Capture())
        got = []
        host.on("await_step", got.append)
        host._dispatch_line(
            line("await_step", {"gateId": "g1", "stepNumber": 3, "lastActionId": "x", "lastSuccess": True})
        )
        self.assertEqual(got[0].gate_id, "g1")
        self.assertEqual(got[0].step_number, 3)
        self.assertEqual(got[0].last_action_id, "x")
        self.assertTrue(got[0].last_success)

    def test_non_json_is_parse_error(self):
        host = Host(stdin=_Capture())
        errs = []
        host.on("parse_error", errs.append)
        host._dispatch_line("not json {")
        self.assertEqual(errs, ["not json {"])

    def test_unknown_type_is_parse_error(self):
        host = Host(stdin=_Capture())
        errs = []
        host.on("parse_error", errs.append)
        host._dispatch_line(line("bogus", {}))
        self.assertEqual(len(errs), 1)


class TestHostSend(unittest.TestCase):
    def test_send_action_result_wire_shape(self):
        cap = _Capture()
        host = Host(stdin=cap)
        host.send_action_result(
            ActionResultMsg(step_id="s1", success=True, state={"notes": []}, data={"id": 1})
        )
        msg = cap.messages()[0]
        self.assertEqual(msg["type"], "action_result")
        self.assertEqual(
            msg["data"], {"stepId": "s1", "success": True, "state": {"notes": []}, "data": {"id": 1}}
        )

    def test_send_action_result_omits_none_fields(self):
        cap = _Capture()
        host = Host(stdin=cap)
        host.send_action_result(ActionResultMsg(step_id="s1", success=True, state={}))
        data = cap.messages()[0]["data"]
        self.assertNotIn("data", data)
        self.assertNotIn("error", data)

    def test_send_approval_and_step(self):
        cap = _Capture()
        host = Host(stdin=cap)
        host.send_approval(ApprovalResponse(step_id="s1", approved=True))
        host.send_step_advance(StepAdvance(gate_id="g1", proceed=False))
        msgs = cap.messages()
        self.assertEqual(msgs[0], {"type": "approval_response", "data": {"stepId": "s1", "approved": True}})
        self.assertEqual(msgs[1], {"type": "step_advance", "data": {"gateId": "g1", "proceed": False}})


class TestMemoryCorrelation(unittest.TestCase):
    def test_recent_memory_round_trip(self):
        cap = _Capture()
        host = Host(stdin=cap)
        result = {}

        def call():
            result["episodes"] = host.recent_memory(limit=5, timeout=2.0)

        t = threading.Thread(target=call)
        t.start()
        # Wait until the request has been written, then echo its requestId back.
        for _ in range(100):
            if cap.messages():
                break
            time.sleep(0.005)
        req = cap.messages()[0]
        self.assertEqual(req["type"], "memory_recent")
        self.assertEqual(req["data"]["limit"], 5)
        rid = req["data"]["requestId"]
        host._dispatch_line(
            line(
                "memory",
                {
                    "requestId": rid,
                    "episodes": [
                        {
                            "tsMs": 1,
                            "actionId": "create_note",
                            "params": "{}",
                            "success": True,
                            "reasoning": "r",
                            "signature": "abcd",
                            "runId": "run1",
                            "goal": "g",
                        }
                    ],
                },
            )
        )
        t.join(timeout=2.0)
        self.assertFalse(t.is_alive())
        eps = result["episodes"]
        self.assertEqual(len(eps), 1)
        self.assertEqual(eps[0].action_id, "create_note")
        self.assertEqual(eps[0].signature, "abcd")

    def test_recent_memory_timeout(self):
        host = Host(stdin=_Capture())
        with self.assertRaises(TimeoutError):
            host.recent_memory(timeout=0.05)

    def test_uncorrelated_memory_is_parse_error(self):
        host = Host(stdin=_Capture())
        errs = []
        host.on("parse_error", errs.append)
        host._dispatch_line(line("memory", {"requestId": "no-such", "episodes": []}))
        self.assertEqual(len(errs), 1)

    def test_memory_error_raises(self):
        cap = _Capture()
        host = Host(stdin=cap)
        result = {}

        def call():
            try:
                host.recent_memory(timeout=2.0)
            except RuntimeError as exc:
                result["error"] = str(exc)

        t = threading.Thread(target=call)
        t.start()
        for _ in range(100):
            if cap.messages():
                break
            time.sleep(0.005)
        rid = cap.messages()[0]["data"]["requestId"]
        host._dispatch_line(line("memory", {"requestId": rid, "episodes": [], "error": "db locked"}))
        t.join(timeout=2.0)
        self.assertEqual(result.get("error"), "db locked")


if __name__ == "__main__":
    unittest.main()
