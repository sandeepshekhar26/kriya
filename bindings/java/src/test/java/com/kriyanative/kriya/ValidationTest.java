package com.kriyanative.kriya;

import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class ValidationTest {
    private static Map<String, Object> obj(Object... kv) {
        Map<String, Object> m = new LinkedHashMap<>();
        for (int i = 0; i < kv.length; i += 2) m.put((String) kv[i], kv[i + 1]);
        return m;
    }

    @Test
    void requiredMissing() {
        Map<String, ParameterSchema> s = P.params();
        s.put("title", P.required(P.str()));
        List<ValidationIssue> issues = Validation.validateParams(obj(), s);
        assertEquals(1, issues.size());
        assertEquals("title", issues.get(0).path);
        assertEquals("required", issues.get(0).message);
    }

    @Test
    void typeMismatch() {
        Map<String, ParameterSchema> s = P.params();
        s.put("id", P.num());
        List<ValidationIssue> issues = Validation.validateParams(obj("id", "x"), s);
        assertEquals(1, issues.size());
        assertTrue(issues.get(0).message.contains("expected number, got string"));
    }

    @Test
    void optionalAbsentIsOk() {
        Map<String, ParameterSchema> s = P.params();
        s.put("note", P.str());
        assertTrue(Validation.validateParams(obj(), s).isEmpty());
    }

    @Test
    void unknownParamsIgnored() {
        Map<String, ParameterSchema> s = P.params();
        s.put("a", P.str());
        assertTrue(Validation.validateParams(obj("a", "x", "extra", 1L), s).isEmpty());
    }

    @Test
    void enumRejectsOutOfSet() {
        Map<String, ParameterSchema> s = P.params();
        s.put("color", P.str().withEnum(new ArrayList<>(List.of("red", "green"))));
        assertTrue(Validation.validateParams(obj("color", "red"), s).isEmpty());
        List<ValidationIssue> bad = Validation.validateParams(obj("color", "blue"), s);
        assertEquals(1, bad.size());
        assertTrue(bad.get(0).message.contains("not one of the allowed"));
    }

    @Test
    void arrayElementTypes() {
        Map<String, ParameterSchema> s = P.params();
        s.put("nums", P.array(P.num()));
        assertTrue(Validation.validateParams(obj("nums", new ArrayList<>(List.of(1L, 2L))), s).isEmpty());
        List<ValidationIssue> bad = Validation.validateParams(obj("nums", new ArrayList<>(List.of(1L, "x"))), s);
        assertEquals(1, bad.size());
        assertEquals("nums[1]", bad.get(0).path);
    }

    @Test
    void nestedObjectRequired() {
        Map<String, ParameterSchema> props = P.params();
        props.put("street", P.required(P.str()));
        Map<String, ParameterSchema> s = P.params();
        s.put("addr", P.obj(props));
        List<ValidationIssue> bad = Validation.validateParams(obj("addr", obj()), s);
        assertEquals(1, bad.size());
        assertEquals("addr.street", bad.get(0).path);
    }

    @Test
    void formatIssuesJoins() {
        List<ValidationIssue> issues = new ArrayList<>();
        issues.add(new ValidationIssue("a", "required"));
        issues.add(new ValidationIssue("b", "expected number, got string"));
        assertEquals("a: required; b: expected number, got string", Validation.formatIssues(issues));
    }
}
