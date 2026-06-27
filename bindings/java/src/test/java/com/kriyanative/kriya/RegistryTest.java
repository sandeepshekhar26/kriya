package com.kriyanative.kriya;

import org.junit.jupiter.api.Test;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class RegistryTest {
    private static Map<String, Object> params(Object... kv) {
        Map<String, Object> m = new LinkedHashMap<>();
        for (int i = 0; i < kv.length; i += 2) m.put((String) kv[i], kv[i + 1]);
        return m;
    }

    @Test
    void registersAndDispatches() {
        Registry r = new Registry();
        Map<String, ParameterSchema> p = P.params();
        p.put("title", P.required(P.str()));
        r.registerAction("create_note", "Create a note.", (params, ctx) -> ActionResult.ok(params.get("title")), p);

        ActionResult res = r.dispatchAction("create_note", params("title", "Hi"), ActionContext.human());
        assertTrue(res.success);
        assertEquals("Hi", res.data);
        assertEquals(List.of("create_note"), r.listActionIds());
    }

    @Test
    void rejectsBadId() {
        Registry r = new Registry();
        assertThrows(ActionValidationException.class,
            () -> r.registerAction("Bad Id", "desc", (p, c) -> ActionResult.ok()));
    }

    @Test
    void rejectsDuplicate() {
        Registry r = new Registry();
        r.registerAction("a", "desc", (p, c) -> ActionResult.ok());
        assertThrows(ActionValidationException.class,
            () -> r.registerAction("a", "desc", (p, c) -> ActionResult.ok()));
    }

    @Test
    void rejectsEmptyDescription() {
        Registry r = new Registry();
        assertThrows(ActionValidationException.class,
            () -> r.registerAction("a", "  ", (p, c) -> ActionResult.ok()));
    }

    @Test
    void rejectsArrayWithoutItems() {
        Registry r = new Registry();
        Map<String, ParameterSchema> p = P.params();
        // an ARRAY schema with null items is invalid
        p.put("tags", new ParameterSchema(ParamType.ARRAY, null, false, null, null, null));
        assertThrows(ActionValidationException.class,
            () -> r.registerAction("a", "desc", (params, c) -> ActionResult.ok(), p));
    }

    @Test
    void dispatchUnknownActionErrs() {
        Registry r = new Registry();
        ActionResult res = r.dispatchAction("nope", params(), ActionContext.human());
        assertFalse(res.success);
        assertTrue(res.error.contains("Unknown action"));
    }

    @Test
    void dispatchValidatesParams() {
        Registry r = new Registry();
        Map<String, ParameterSchema> p = P.params();
        p.put("id", P.required(P.num()));
        r.registerAction("del", "Delete.", (params, c) -> ActionResult.ok(), p);

        ActionResult res = r.dispatchAction("del", params("id", "not-a-number"), ActionContext.human());
        assertFalse(res.success);
        assertTrue(res.error.contains("Invalid parameters"));
    }

    @Test
    void handlerThrowBecomesErr() {
        Registry r = new Registry();
        r.registerAction("boom", "Throws.", (p, c) -> { throw new IllegalStateException("kaboom"); });
        ActionResult res = r.dispatchAction("boom", params(), ActionContext.human());
        assertFalse(res.success);
        assertEquals("kaboom", res.error);
    }

    @Test
    void wrapActionOkAndThrow() {
        Registry r = new Registry();
        r.wrapAction(args -> ((Integer) args[0]) * 2, "double_it", "Doubles.", P.params(),
            p -> new Object[] { 21 }, res -> res);
        ActionResult ok = r.dispatchAction("double_it", params(), ActionContext.human());
        assertTrue(ok.success);
        assertEquals(42, ok.data);

        r.wrapAction(args -> { throw new RuntimeException("nope"); }, "fails", "Fails.", P.params(), null, null);
        ActionResult err = r.dispatchAction("fails", params(), ActionContext.human());
        assertFalse(err.success);
        assertEquals("nope", err.error);
    }

    @Test
    void compositionViaCtxCall() {
        Registry r = new Registry();
        r.registerAction("child", "Child.", (p, c) -> ActionResult.ok("child-ran"));
        r.registerAction("parent", "Parent.", (p, c) -> c.call.call("child", params()));
        ActionResult res = r.dispatchAction("parent", params(), ActionContext.human());
        assertTrue(res.success);
        assertEquals("child-ran", res.data);
    }

    @Test
    void compositionCycleDetected() {
        Registry r = new Registry();
        r.registerAction("a", "A.", (p, c) -> c.call.call("b", params()));
        r.registerAction("b", "B.", (p, c) -> c.call.call("a", params()));
        ActionResult res = r.dispatchAction("a", params(), ActionContext.human());
        assertFalse(res.success);
        assertTrue(res.error.contains("cycle"));
    }

    @Test
    void toolSchemasShape() {
        Registry r = new Registry();
        Map<String, ParameterSchema> p = P.params();
        p.put("title", P.required(P.str()));
        r.registerAction("create_note", "Create a note.", (params, c) -> ActionResult.ok(), p, List.of("write:notes"), 2);

        List<Map<String, Object>> tools = r.toolSchemas();
        assertEquals(1, tools.size());
        Map<String, Object> t = tools.get(0);
        assertEquals("create_note", t.get("name"));
        assertEquals(2, t.get("version"));
        assertEquals(List.of("write:notes"), t.get("permissions"));
        assertTrue(t.containsKey("inputSchema"));

        // mcp shape drops permissions
        Map<String, Object> mcp = r.mcpToolSchemas().get(0);
        assertFalse(mcp.containsKey("permissions"));
        assertEquals("create_note", mcp.get("name"));
    }
}
