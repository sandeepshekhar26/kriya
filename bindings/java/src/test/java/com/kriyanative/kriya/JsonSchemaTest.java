package com.kriyanative.kriya;

import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class JsonSchemaTest {
    @Test
    void scalarWithDescription() {
        Map<String, Object> s = JsonSchema.toJsonSchema(P.str().withDescription("the title"));
        assertEquals("string", s.get("type"));
        assertEquals("the title", s.get("description"));
    }

    @Test
    void arrayCarriesItems() {
        Map<String, Object> s = JsonSchema.toJsonSchema(P.array(P.num()));
        assertEquals("array", s.get("type"));
        @SuppressWarnings("unchecked")
        Map<String, Object> items = (Map<String, Object>) s.get("items");
        assertEquals("number", items.get("type"));
    }

    @Test
    void enumExported() {
        Map<String, Object> s = JsonSchema.toJsonSchema(P.str().withEnum(new ArrayList<>(List.of("a", "b"))));
        assertEquals(List.of("a", "b"), s.get("enum"));
    }

    @Test
    void paramsToTopLevelObjectWithRequiredArray() {
        Map<String, ParameterSchema> p = P.params();
        p.put("title", P.required(P.str()));
        p.put("body", P.str());
        Map<String, Object> schema = JsonSchema.paramsToJsonSchema(p);

        assertEquals("object", schema.get("type"));
        @SuppressWarnings("unchecked")
        Map<String, Object> props = (Map<String, Object>) schema.get("properties");
        assertTrue(props.containsKey("title"));
        assertTrue(props.containsKey("body"));
        // required is an ARRAY on the parent (JSON Schema draft 2020-12), not a per-prop bool
        assertEquals(List.of("title"), schema.get("required"));
    }

    @Test
    void noRequiredKeyWhenNonePresent() {
        Map<String, ParameterSchema> p = P.params();
        p.put("body", P.str());
        Map<String, Object> schema = JsonSchema.paramsToJsonSchema(p);
        assertFalse(schema.containsKey("required"));
    }

    @Test
    void nestedObjectProperties() {
        Map<String, ParameterSchema> inner = P.params();
        inner.put("street", P.required(P.str()));
        Map<String, Object> s = JsonSchema.toJsonSchema(P.obj(inner));
        assertEquals("object", s.get("type"));
        @SuppressWarnings("unchecked")
        Map<String, Object> props = (Map<String, Object>) s.get("properties");
        assertTrue(props.containsKey("street"));
        assertEquals(List.of("street"), s.get("required"));
    }
}
