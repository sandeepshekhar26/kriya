package com.kriyanative.kriya;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** The definition passed to {@code Registry.registerAction}. */
public final class ActionDefinition {
    public final String id;
    public final String description;
    public final Handler handler;
    public final Map<String, ParameterSchema> parameters;
    public final List<String> permissions;
    public final int version;

    ActionDefinition(String id, String description, Handler handler,
                     Map<String, ParameterSchema> parameters, List<String> permissions, int version) {
        this.id = id;
        this.description = description;
        this.handler = handler;
        this.parameters = Collections.unmodifiableMap(
            parameters == null ? new LinkedHashMap<>() : new LinkedHashMap<>(parameters));
        this.permissions = Collections.unmodifiableList(
            permissions == null ? Collections.emptyList() : new java.util.ArrayList<>(permissions));
        this.version = version;
    }
}
