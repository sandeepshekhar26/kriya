package com.kriyanative.kriya;

import org.junit.jupiter.api.Test;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class JsonTest {
    @Test
    void parsesPrimitives() {
        assertEquals("hi", Json.parse("\"hi\""));
        assertEquals(42L, Json.parse("42"));
        assertEquals(-7L, Json.parse("-7"));
        assertEquals(3.5, Json.parse("3.5"));
        assertEquals(Boolean.TRUE, Json.parse("true"));
        assertEquals(Boolean.FALSE, Json.parse("false"));
        assertNull(Json.parse("null"));
    }

    @Test
    void parsesNestedObjectAndArray() {
        @SuppressWarnings("unchecked")
        Map<String, Object> m = (Map<String, Object>) Json.parse("{\"a\":1,\"b\":[true,\"x\",null],\"c\":{\"d\":2.0}}");
        assertEquals(1L, m.get("a"));
        assertEquals(List.of(Boolean.TRUE, "x"), ((List<?>) m.get("b")).subList(0, 2));
        assertNull(((List<?>) m.get("b")).get(2));
        @SuppressWarnings("unchecked")
        Map<String, Object> c = (Map<String, Object>) m.get("c");
        assertEquals(2L, ((Number) c.get("d")).longValue());
    }

    @Test
    void roundTripsCompact() {
        String src = "{\"name\":\"create_note\",\"version\":1,\"nested\":{\"k\":[1,2,3]}}";
        assertEquals(src, Json.stringify(Json.parse(src)));
    }

    @Test
    void handlesStringEscapes() {
        Object v = Json.parse("\"a\\\"b\\n\\t\\u0041\"");
        assertEquals("a\"b\n\tA", v);
        // and re-serialises control chars
        assertEquals("\"a\\\"b\\n\"", Json.stringify("a\"b\n"));
    }

    @Test
    void rendersIntegralDoubleWithoutTrailingZero() {
        assertEquals("2", Json.stringify(2.0));
        assertEquals("2.5", Json.stringify(2.5));
        assertEquals("2", Json.stringify(2L));
        assertEquals("-3", Json.stringify(-3.0));
    }

    @Test
    void preservesKeyOrder() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("z", 1L);
        m.put("a", 2L);
        assertEquals("{\"z\":1,\"a\":2}", Json.stringify(m));
    }

    @Test
    void rejectsMalformed() {
        assertThrows(Json.JsonException.class, () -> Json.parse("{"));
        assertThrows(Json.JsonException.class, () -> Json.parse("[1,]"));
        assertThrows(Json.JsonException.class, () -> Json.parse("nul"));
        assertThrows(Json.JsonException.class, () -> Json.parse("1 2"));
    }

    @Test
    void parseObjectRejectsNonObject() {
        assertThrows(Json.JsonException.class, () -> Json.parseObject("[1,2]"));
    }
}
