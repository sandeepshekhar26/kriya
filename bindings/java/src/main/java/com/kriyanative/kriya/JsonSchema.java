package com.kriyanative.kriya;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Standards-compliant JSON Schema (draft 2020-12 clean) export. kriya's compact
 * {@link ParameterSchema} uses a per-property {@code required} bool and may carry a bare item type;
 * MCP clients want {@code required} as an array on the parent object and {@code items} as a schema
 * object. This converts between them, faithful to kriya-core's {@code jsonschema.ts}.
 */
public final class JsonSchema {
    private JsonSchema() {}

    static String typeName(ParamType t) {
        switch (t) {
            case STRING: return "string";
            case NUMBER: return "number";
            case BOOLEAN: return "boolean";
            case ARRAY: return "array";
            case OBJECT: return "object";
            default: return "object";
        }
    }

    /** Convert one {@link ParameterSchema} node to a JSON Schema object. */
    public static Map<String, Object> toJsonSchema(ParameterSchema schema) {
        Map<String, Object> node = new LinkedHashMap<>();
        node.put("type", typeName(schema.type));

        if (schema.description != null) node.put("description", schema.description);

        if (schema.enumValues != null) {
            node.put("enum", new ArrayList<>(schema.enumValues));
        }

        if (schema.type == ParamType.ARRAY) {
            // items is guaranteed non-null for arrays by the registry validator.
            node.put("items", schema.items != null ? toJsonSchema(schema.items) : new LinkedHashMap<>());
        } else if (schema.type == ParamType.OBJECT && schema.properties != null) {
            Map<String, Object> props = new LinkedHashMap<>();
            List<Object> required = new ArrayList<>();
            for (Map.Entry<String, ParameterSchema> e : schema.properties.entrySet()) {
                props.put(e.getKey(), toJsonSchema(e.getValue()));
                if (e.getValue().required) required.add(e.getKey());
            }
            node.put("properties", props);
            if (!required.isEmpty()) node.put("required", required);
        }

        return node;
    }

    /** Build a top-level object schema from a flat map of named parameter schemas. */
    public static Map<String, Object> paramsToJsonSchema(Map<String, ParameterSchema> parameters) {
        Map<String, Object> props = new LinkedHashMap<>();
        List<Object> required = new ArrayList<>();
        for (Map.Entry<String, ParameterSchema> e : parameters.entrySet()) {
            props.put(e.getKey(), toJsonSchema(e.getValue()));
            if (e.getValue().required) required.add(e.getKey());
        }
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("type", "object");
        result.put("properties", props);
        if (!required.isEmpty()) result.put("required", required);
        return result;
    }
}
