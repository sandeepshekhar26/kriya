package com.kriyanative.kriya;

/** One problem found validating params against their schemas. */
public final class ValidationIssue {
    public final String path;
    public final String message;

    public ValidationIssue(String path, String message) {
        this.path = path;
        this.message = message;
    }

    @Override
    public String toString() { return path + ": " + message; }
}
