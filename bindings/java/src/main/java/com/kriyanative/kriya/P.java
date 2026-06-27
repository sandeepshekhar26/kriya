package com.kriyanative.kriya;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Ready-made parameter-schema helpers — the JVM equivalent of kriya-core's {@code str}/{@code num}.
 * Each call returns a fresh immutable {@link ParameterSchema}; mark required with {@link #required}.
 */
public final class P {
    private P() {}

    public static ParameterSchema str() { return new ParameterSchema(ParamType.STRING, null, false, null, null, null); }
    public static ParameterSchema num() { return new ParameterSchema(ParamType.NUMBER, null, false, null, null, null); }
    public static ParameterSchema bool() { return new ParameterSchema(ParamType.BOOLEAN, null, false, null, null, null); }

    public static ParameterSchema array(ParameterSchema items) { return array(items, null, false); }
    public static ParameterSchema array(ParameterSchema items, String description, boolean required) {
        return new ParameterSchema(ParamType.ARRAY, description, required, items, null, null);
    }

    public static ParameterSchema obj(Map<String, ParameterSchema> properties) { return obj(properties, null, false); }
    public static ParameterSchema obj(Map<String, ParameterSchema> properties, String description, boolean required) {
        return new ParameterSchema(ParamType.OBJECT, description, required, null, new LinkedHashMap<>(properties), null);
    }

    /** Mark a schema required, e.g. {@code P.required(P.str())}. */
    public static ParameterSchema required(ParameterSchema schema) { return schema.asRequired(); }

    /** Convenience: an ordered map for an action's parameters. */
    public static Map<String, ParameterSchema> params() { return new LinkedHashMap<>(); }

    /** Restrict a schema to an enumerated set of allowed values. */
    public static ParameterSchema oneOf(ParameterSchema schema, List<Object> values) { return schema.withEnum(values); }
}
