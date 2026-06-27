package com.kriyanative.kriya;

import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;
import java.util.regex.Pattern;

/**
 * The action registry: a map of registered actions. Hands the host an MCP-compatible tool schema of
 * every action (no handlers) and dispatches an incoming agent tool-call to the right handler — through
 * the same validation + composition path as kriya-core. A faithful JVM port of the verified
 * Python/.NET bindings.
 */
public final class Registry {
    /** Max nested-composition depth (a handler calling a child calling another…). */
    public static final int MAX_COMPOSE_DEPTH = 8;

    private static final Pattern ID_PATTERN = Pattern.compile("^[a-z][a-z0-9_]*$");
    private final Map<String, ActionDefinition> actions = new LinkedHashMap<>();

    /** Register an action so agents can discover and call it. */
    public RegisteredAction registerAction(String id, String description, Handler handler) {
        return registerAction(id, description, handler, null, null, 1);
    }

    public RegisteredAction registerAction(String id, String description, Handler handler,
                                           Map<String, ParameterSchema> parameters) {
        return registerAction(id, description, handler, parameters, null, 1);
    }

    public RegisteredAction registerAction(String id, String description, Handler handler,
                                           Map<String, ParameterSchema> parameters,
                                           List<String> permissions, int version) {
        ActionDefinition def = new ActionDefinition(id, description, handler, parameters, permissions, version);
        validateDefinition(def);
        actions.put(def.id, def);
        return new RegisteredAction(this, def.id);
    }

    /**
     * Bolt the kriya layer onto a function the app <em>already has</em> — augment, not migrate.
     * {@code fn} takes the mapped argument array and returns a plain value (or throws); a returned
     * value becomes {@code ok(data)}, a throw becomes {@code err(message)}.
     */
    public RegisteredAction wrapAction(Function<Object[], Object> fn, String id, String description,
                                       Map<String, ParameterSchema> parameters,
                                       List<String> permissions, int version,
                                       Function<Map<String, Object>, Object[]> mapParams,
                                       Function<Object, Object> mapResult) {
        Handler handler = (params, ctx) -> {
            Object returned;
            try {
                Object[] args = mapParams != null ? mapParams.apply(params) : new Object[] { params };
                returned = fn.apply(args);
            } catch (Exception ex) {
                // The app's own throw is an expected failure mode, not a crash.
                return ActionResult.err(ex.getMessage() == null ? ex.toString() : ex.getMessage());
            }
            Object data = mapResult != null ? mapResult.apply(returned) : returned;
            return ActionResult.ok(data);
        };
        return registerAction(id, description, handler, parameters, permissions, version);
    }

    /** Convenience overload: wrap a function with no permissions, version 1, default param/result mapping. */
    public RegisteredAction wrapAction(Function<Object[], Object> fn, String id, String description,
                                       Map<String, ParameterSchema> parameters,
                                       Function<Map<String, Object>, Object[]> mapParams,
                                       Function<Object, Object> mapResult) {
        return wrapAction(fn, id, description, parameters, null, 1, mapParams, mapResult);
    }

    private void validateDefinition(ActionDefinition def) {
        if (def.id == null || !ID_PATTERN.matcher(def.id).matches())
            throw new ActionValidationException("Action id \"" + def.id + "\" is invalid. Use snake_case starting with a letter.");
        if (actions.containsKey(def.id))
            throw new ActionValidationException("Action \"" + def.id + "\" is already registered.");
        if (def.description == null || def.description.trim().isEmpty())
            throw new ActionValidationException("Action \"" + def.id + "\" needs a non-empty description.");
        for (Map.Entry<String, ParameterSchema> e : def.parameters.entrySet()) {
            if (e.getValue().type == ParamType.ARRAY && e.getValue().items == null)
                throw new ActionValidationException("Action \"" + def.id + "\" parameter \"" + e.getKey() + "\" is an array but has no items.");
        }
    }

    /** Look up and run a registered action by id (used when an agent calls a tool). */
    public ActionResult dispatchAction(String id, Map<String, Object> parameters, ActionContext ctx) {
        return dispatchWithChain(id, parameters, ctx, ctx.chain == null ? Collections.emptyList() : ctx.chain);
    }

    private ActionResult dispatchWithChain(String id, Map<String, Object> parameters, ActionContext ctx, List<String> parentChain) {
        if (parentChain.contains(id)) {
            List<String> path = new ArrayList<>(parentChain);
            path.add(id);
            return ActionResult.err("Composition cycle detected: " + String.join(" -> ", path) + ".");
        }
        if (parentChain.size() >= MAX_COMPOSE_DEPTH)
            return ActionResult.err("Composition depth " + MAX_COMPOSE_DEPTH + " exceeded at \"" + id + "\" (chain: " + String.join(" -> ", parentChain) + ").");

        ActionDefinition def = actions.get(id);
        if (def == null) return ActionResult.err("Unknown action \"" + id + "\".");

        Map<String, Object> params = parameters == null ? new LinkedHashMap<>() : parameters;
        List<ValidationIssue> issues = Validation.validateParams(params, def.parameters);
        if (!issues.isEmpty())
            return ActionResult.err("Invalid parameters: " + Validation.formatIssues(issues) + ".");

        List<String> newChain = new ArrayList<>(parentChain);
        newChain.add(id);
        ActionContext.ChildCall childCall = (childId, childParams) ->
            dispatchWithChain(childId, childParams,
                new ActionContext(ctx.caller, ctx.stepId, newChain, null), newChain);
        ActionContext childCtx = new ActionContext(ctx.caller, ctx.stepId, newChain, childCall);

        try {
            return def.handler.handle(params, childCtx);
        } catch (Exception ex) {
            return ActionResult.err(ex.getMessage() == null ? ex.toString() : ex.getMessage());
        }
    }

    /** MCP-compatible tool schemas for every registered action (no handlers). camelCase wire keys. */
    public List<Map<String, Object>> toolSchemas() {
        List<Map<String, Object>> list = new ArrayList<>();
        for (ActionDefinition def : actions.values()) {
            Map<String, Object> tool = new LinkedHashMap<>();
            tool.put("name", def.id);
            tool.put("version", def.version);
            tool.put("description", def.description);
            tool.put("permissions", new ArrayList<Object>(def.permissions));
            tool.put("inputSchema", JsonSchema.paramsToJsonSchema(def.parameters));
            list.add(tool);
        }
        return list;
    }

    /** Like {@link #toolSchemas} but the bare MCP shape (no kriya {@code permissions}). */
    public List<Map<String, Object>> mcpToolSchemas() {
        List<Map<String, Object>> list = new ArrayList<>();
        for (Map<String, Object> t : toolSchemas()) {
            Map<String, Object> tool = new LinkedHashMap<>();
            tool.put("name", t.get("name"));
            tool.put("version", t.get("version"));
            tool.put("description", t.get("description"));
            tool.put("inputSchema", t.get("inputSchema"));
            list.add(tool);
        }
        return list;
    }

    public List<String> listActionIds() { return new ArrayList<>(actions.keySet()); }
    public ActionDefinition get(String id) { return actions.get(id); }
    public void clear() { actions.clear(); }
}
