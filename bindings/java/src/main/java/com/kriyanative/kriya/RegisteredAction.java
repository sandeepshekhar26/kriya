package com.kriyanative.kriya;

import java.util.Map;

/** A callable handle returned by {@link Registry#registerAction} (handy for tests + direct calls). */
public final class RegisteredAction {
    private final Registry registry;
    public final String id;

    RegisteredAction(Registry registry, String id) {
        this.registry = registry;
        this.id = id;
    }

    public ActionResult call(Map<String, Object> params) {
        return registry.dispatchAction(id, params, ActionContext.human());
    }

    public ActionResult call(Map<String, Object> params, ActionContext ctx) {
        return registry.dispatchAction(id, params, ctx);
    }
}
