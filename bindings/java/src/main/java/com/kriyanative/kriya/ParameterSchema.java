package com.kriyanative.kriya;

import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * The schema for a single action parameter. Immutable; build schemas with the {@link P} helpers and
 * the {@code asRequired}/{@code withDescription} copies. Mirrors kriya-core's {@code ParameterSchema}.
 */
public final class ParameterSchema {
    public final ParamType type;
    public final String description;             // nullable
    public final boolean required;
    public final ParameterSchema items;          // for ARRAY: the element schema (nullable otherwise)
    public final Map<String, ParameterSchema> properties; // for OBJECT (nullable otherwise)
    public final List<Object> enumValues;        // restrict to a set of values (nullable)

    ParameterSchema(ParamType type, String description, boolean required,
                    ParameterSchema items, Map<String, ParameterSchema> properties, List<Object> enumValues) {
        this.type = type;
        this.description = description;
        this.required = required;
        this.items = items;
        this.properties = properties == null ? null : Collections.unmodifiableMap(properties);
        this.enumValues = enumValues == null ? null : Collections.unmodifiableList(enumValues);
    }

    /** Return a copy marked required. */
    public ParameterSchema asRequired() {
        return new ParameterSchema(type, description, true, items, properties, enumValues);
    }

    /** Return a copy with the given description. */
    public ParameterSchema withDescription(String description) {
        return new ParameterSchema(type, description, required, items, properties, enumValues);
    }

    /** Return a copy restricted to an enumerated set of allowed values. */
    public ParameterSchema withEnum(List<Object> values) {
        return new ParameterSchema(type, description, required, items, properties, values);
    }
}
