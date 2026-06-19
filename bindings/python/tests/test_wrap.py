"""wrap_action parity with kriya-core's wrap.ts -- adopt an existing function, no rewrite."""

import unittest

from kriya import ActionContext, Registry, required, string


class TestWrapAction(unittest.TestCase):
    def setUp(self):
        self.reg = Registry()

    def test_return_value_becomes_data(self):
        def create_note(params):
            return {"id": 1, "title": params["title"]}

        self.reg.wrap_action(
            create_note,
            id="create_note",
            description="Create a note.",
            parameters={"title": required(string)},
        )
        res = self.reg.dispatch_action(
            "create_note", {"title": "hi"}, ActionContext(caller="agent")
        )
        self.assertTrue(res.success)
        self.assertEqual(res.data, {"id": 1, "title": "hi"})

    def test_thrown_error_becomes_failed_result(self):
        def explode(_params):
            raise RuntimeError("nope")

        self.reg.wrap_action(explode, id="explode", description="d")
        res = self.reg.dispatch_action("explode", {}, ActionContext())
        self.assertFalse(res.success)
        self.assertEqual(res.error, "nope")

    def test_map_params_to_positional_args(self):
        seen = {}

        def create(title, body):  # positional args, like a function the app already has
            seen["args"] = (title, body)
            return "ok"

        self.reg.wrap_action(
            create,
            id="create",
            description="d",
            parameters={"title": required(string), "body": string},
            map_params=lambda p: [p["title"], p.get("body", "")],
        )
        self.reg.dispatch_action(
            "create", {"title": "T", "body": "B"}, ActionContext()
        )
        self.assertEqual(seen["args"], ("T", "B"))

    def test_map_result_transform(self):
        def create(_params):
            return {"id": 7, "secret": "x"}

        self.reg.wrap_action(
            create,
            id="create",
            description="d",
            map_result=lambda r: {"id": r["id"]},  # hide the secret from the agent
        )
        res = self.reg.dispatch_action("create", {}, ActionContext())
        self.assertEqual(res.data, {"id": 7})


if __name__ == "__main__":
    unittest.main()
