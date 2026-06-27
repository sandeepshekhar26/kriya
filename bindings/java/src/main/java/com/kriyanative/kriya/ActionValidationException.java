package com.kriyanative.kriya;

/** Thrown when an action <em>definition</em> is invalid (bad id, duplicate, missing description…). */
public final class ActionValidationException extends RuntimeException {
    public ActionValidationException(String message) { super(message); }
}
