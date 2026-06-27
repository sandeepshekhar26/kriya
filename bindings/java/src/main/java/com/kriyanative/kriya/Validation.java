package com.kriyanative.kriya;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Runtime validation of action parameters against their declared schemas — what stops an agent (or a
 * buggy caller) invoking a handler with the wrong shape. Types, enums, required-ness, array element
 * and object property types. Unknown params are ignored (forward-compatible). Faithful port of
 * kriya-core's {@code validate.ts}.
 */
public final class Validation {
    private Validation() {}

    private static String typeOf(Object node) {
        if (node == null) return "null";
        if (node instanceof String) return "string";
        if (node instanceof Boolean) return "boolean";
        if (node instanceof Number) return "number";
        if (node instanceof List) return "array";
        if (node instanceof Map) return "object";
        return "object";
    }

    @SuppressWarnings("unchecked")
    private static void check(Object value, ParameterSchema schema, String path, List<ValidationIssue> issues) {
        String actual = typeOf(value);
        String expected = JsonSchema.typeName(schema.type);
        if (!actual.equals(expected)) {
            issues.add(new ValidationIssue(path, "expected " + expected + ", got " + actual));
            return; // type wrong — deeper checks would be noise
        }

        if (schema.enumValues != null) {
            String asJson = Json.stringify(value);
            boolean ok = false;
            for (Object e : schema.enumValues) {
                if (Json.stringify(e).equals(asJson)) { ok = true; break; }
            }
            if (!ok) issues.add(new ValidationIssue(path, "value " + asJson + " is not one of the allowed values"));
        }

        if (schema.type == ParamType.ARRAY && schema.items != null && value instanceof List) {
            List<Object> arr = (List<Object>) value;
            for (int i = 0; i < arr.size(); i++) {
                check(arr.get(i), schema.items, path + "[" + i + "]", issues);
            }
        }

        if (schema.type == ParamType.OBJECT && schema.properties != null && value instanceof Map) {
            Map<String, Object> obj = (Map<String, Object>) value;
            for (Map.Entry<String, ParameterSchema> e : schema.properties.entrySet()) {
                String key = e.getKey();
                ParameterSchema prop = e.getValue();
                if (!obj.containsKey(key)) {
                    if (prop.required) issues.add(new ValidationIssue(path + "." + key, "required"));
                    continue;
                }
                check(obj.get(key), prop, path + "." + key, issues);
            }
        }
    }

    /** Validate a params object against a map of named parameter schemas. */
    public static List<ValidationIssue> validateParams(Map<String, Object> parameters, Map<String, ParameterSchema> schemas) {
        List<ValidationIssue> issues = new ArrayList<>();
        for (Map.Entry<String, ParameterSchema> e : schemas.entrySet()) {
            String name = e.getKey();
            ParameterSchema schema = e.getValue();
            if (parameters == null || !parameters.containsKey(name)) {
                if (schema.required) issues.add(new ValidationIssue(name, "required"));
                continue;
            }
            check(parameters.get(name), schema, name, issues);
        }
        return issues;
    }

    /** Format issues into a single human/agent-readable string. */
    public static String formatIssues(List<ValidationIssue> issues) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < issues.size(); i++) {
            if (i > 0) sb.append("; ");
            sb.append(issues.get(i).path).append(": ").append(issues.get(i).message);
        }
        return sb.toString();
    }
}
