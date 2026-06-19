"""Registry, dispatch, validation, and composition parity with kriya-core's registry.ts."""

import unittest

from kriya import (
    ActionContext,
    ActionValidationError,
    Registry,
    array,
    ok,
    required,
    string,
)


class TestRegistry(unittest.TestCase):
    def setUp(self):
        self.reg = Registry()

    def test_register_and_call(self):
        handle = self.reg.register_action(
            id="greet",
            description="Greet someone.",
            parameters={"name": required(string)},
            handler=lambda p, ctx: ok({"hello": p["name"]}),
        )
        res = handle.call({"name": "Sandeep"})
        self.assertTrue(res.success)
        self.assertEqual(res.data, {"hello": "Sandeep"})

    def test_duplicate_id_rejected(self):
        self.reg.register_action(id="a", description="d", handler=lambda p, c: ok())
        with self.assertRaises(ActionValidationError):
            self.reg.register_action(id="a", description="d", handler=lambda p, c: ok())

    def test_bad_id_rejected(self):
        for bad in ("Bad", "1x", "with-dash", ""):
            with self.assertRaises(ActionValidationError):
                self.reg.register_action(
                    id=bad, description="d", handler=lambda p, c: ok()
                )

    def test_empty_description_rejected(self):
        with self.assertRaises(ActionValidationError):
            self.reg.register_action(id="a", description="  ", handler=lambda p, c: ok())

    def test_array_without_items_rejected(self):
        from kriya import ParameterSchema

        with self.assertRaises(ActionValidationError):
            self.reg.register_action(
                id="a",
                description="d",
                parameters={"tags": ParameterSchema("array")},
                handler=lambda p, c: ok(),
            )

    def test_dispatch_unknown_action(self):
        res = self.reg.dispatch_action("nope", {}, ActionContext())
        self.assertFalse(res.success)
        self.assertIn('Unknown action "nope"', res.error)

    def test_dispatch_invalid_params(self):
        self.reg.register_action(
            id="a",
            description="d",
            parameters={"n": required(string)},
            handler=lambda p, c: ok(),
        )
        res = self.reg.dispatch_action("a", {}, ActionContext(caller="agent"))
        self.assertFalse(res.success)
        self.assertIn("Invalid parameters", res.error)

    def test_handler_exception_becomes_failed_result(self):
        def boom(p, c):
            raise ValueError("kaboom")

        self.reg.register_action(id="a", description="d", handler=boom)
        res = self.reg.dispatch_action("a", {}, ActionContext())
        self.assertFalse(res.success)
        self.assertEqual(res.error, "kaboom")

    def test_composition(self):
        self.reg.register_action(
            id="child",
            description="child",
            parameters={"x": required(string)},
            handler=lambda p, c: ok({"got": p["x"]}),
        )

        def parent(p, ctx):
            child = ctx.call("child", {"x": "from-parent"})
            return ok({"child": child.data})

        self.reg.register_action(id="parent", description="parent", handler=parent)
        res = self.reg.dispatch_action("parent", {}, ActionContext(caller="agent"))
        self.assertTrue(res.success)
        self.assertEqual(res.data, {"child": {"got": "from-parent"}})

    def test_cycle_detected(self):
        def a(p, ctx):
            return ctx.call("a", {})

        self.reg.register_action(id="a", description="d", handler=a)
        res = self.reg.dispatch_action("a", {}, ActionContext())
        self.assertFalse(res.success)
        self.assertIn("Composition cycle detected", res.error)

    def test_depth_cap(self):
        # n0 -> n1 -> ... each calls the next; the chain must trip MAX_COMPOSE_DEPTH.
        for i in range(12):
            nxt = f"n{i + 1}"

            def make(nxt_id):
                return lambda p, ctx: (
                    ctx.call(nxt_id, {}) if ctx.call else ok()
                )

            self.reg.register_action(
                id=f"n{i}", description="chain", handler=make(nxt)
            )
        res = self.reg.dispatch_action("n0", {}, ActionContext())
        self.assertFalse(res.success)
        self.assertIn("depth", res.error.lower())

    def test_list_action_ids_and_clear(self):
        self.reg.register_action(id="a", description="d", handler=lambda p, c: ok())
        self.assertEqual(self.reg.list_action_ids(), ["a"])
        self.reg.clear()
        self.assertEqual(self.reg.list_action_ids(), [])


if __name__ == "__main__":
    unittest.main()
