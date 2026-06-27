package com.kriyanative.kriya;

import java.util.Map;

/** An action handler: receives the agent's params + context, returns an {@link ActionResult}. */
@FunctionalInterface
public interface Handler {
    ActionResult handle(Map<String, Object> params, ActionContext ctx);
}
