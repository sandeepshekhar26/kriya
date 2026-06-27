package com.kriyanative.kriya;

import java.util.Collections;
import java.util.List;
import java.util.Map;

/** Context handed to a handler at execution time. */
public final class ActionContext {
    /** Invoke a child action from inside a handler (same validation + audit path). */
    @FunctionalInterface
    public interface ChildCall {
        ActionResult call(String actionId, Map<String, Object> params);
    }

    /** "human" or "agent". */
    public final String caller;
    /** The agent step id this call belongs to, when {@link #caller} is "agent" (nullable). */
    public final String stepId;
    /** Action ids the current call descends from, parent-first (cycle/depth guard, read-only). */
    public final List<String> chain;
    /** Composition entry point (nullable on the outermost human/agent call). */
    public final ChildCall call;

    ActionContext(String caller, String stepId, List<String> chain, ChildCall call) {
        this.caller = caller == null ? "human" : caller;
        this.stepId = stepId;
        this.chain = chain == null ? Collections.emptyList() : Collections.unmodifiableList(chain);
        this.call = call;
    }

    /** A plain human-initiated context. */
    public static ActionContext human() { return new ActionContext("human", null, null, null); }

    /** An agent-initiated context for the given step. */
    public static ActionContext agent(String stepId) { return new ActionContext("agent", stepId, null, null); }
}
