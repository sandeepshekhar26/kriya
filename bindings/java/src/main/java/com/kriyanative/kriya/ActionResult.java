package com.kriyanative.kriya;

/** What an action handler returns. {@link #error} is set only when not successful. */
public final class ActionResult {
    public final boolean success;
    public final Object data;   // nullable
    public final String error;  // nullable

    private ActionResult(boolean success, Object data, String error) {
        this.success = success;
        this.data = data;
        this.error = error;
    }

    public static ActionResult ok() { return new ActionResult(true, null, null); }
    public static ActionResult ok(Object data) { return new ActionResult(true, data, null); }
    public static ActionResult err(String message) { return new ActionResult(false, null, message); }
}
