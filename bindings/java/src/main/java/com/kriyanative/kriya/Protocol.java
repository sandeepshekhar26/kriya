package com.kriyanative.kriya;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The agent-loop wire protocol the JVM binding speaks to {@code kriya-host} over stdio. Each line is a
 * JSON object {@code {"type": ..., "data": {...}}}. {@code data} fields are camelCase (the Rust structs
 * use {@code rename_all="camelCase"}), except {@code done} whose fields are already lowercase. These
 * nested types use idiomatic Java names and convert at the edge via {@code fromWire}/{@code toWire} —
 * mirrors protocol.ts and the Python/.NET bindings.
 */
public final class Protocol {
    private Protocol() {}

    // ── wire accessors ────────────────────────────────────────────────────────
    static String str(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof String ? (String) v : ""; }
    static String strOrNull(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof String ? (String) v : null; }
    @SuppressWarnings("unchecked")
    static Map<String, Object> obj(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof Map ? (Map<String, Object>) v : new LinkedHashMap<>(); }
    static boolean bool(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof Boolean && (Boolean) v; }
    static Boolean boolOrNull(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof Boolean ? (Boolean) v : null; }
    static long lng(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof Number ? ((Number) v).longValue() : 0L; }
    static int integer(Map<String, Object> d, String k) { Object v = d.get(k); return v instanceof Number ? ((Number) v).intValue() : 0; }

    // ── host → app ──────────────────────────────────────────────────────────
    /** host → app: run this action and reply with an {@link ActionResultMsg} (same stepId). */
    public static final class ActionRequest {
        public final String stepId, actionId, reasoning;
        public final Map<String, Object> params;
        ActionRequest(String stepId, String actionId, Map<String, Object> params, String reasoning) {
            this.stepId = stepId; this.actionId = actionId; this.params = params; this.reasoning = reasoning;
        }
        static ActionRequest fromWire(Map<String, Object> d) {
            return new ActionRequest(str(d, "stepId"), str(d, "actionId"), obj(d, "params"), str(d, "reasoning"));
        }
    }

    /** host → app: a guarded action needs a human's go-ahead. Reply with {@link ApprovalResponse}. */
    public static final class ApprovalRequest {
        public final String stepId, actionId, reasoning;
        public final Map<String, Object> params;
        ApprovalRequest(String stepId, String actionId, Map<String, Object> params, String reasoning) {
            this.stepId = stepId; this.actionId = actionId; this.params = params; this.reasoning = reasoning;
        }
        static ApprovalRequest fromWire(Map<String, Object> d) {
            return new ApprovalRequest(str(d, "stepId"), str(d, "actionId"), obj(d, "params"), str(d, "reasoning"));
        }
    }

    /** host → app: paused before a step (step-mode). Reply with {@link StepAdvance}. */
    public static final class AwaitStep {
        public final String gateId;
        public final int stepNumber;
        public final String lastActionId;   // nullable
        public final Boolean lastSuccess;    // nullable
        AwaitStep(String gateId, int stepNumber, String lastActionId, Boolean lastSuccess) {
            this.gateId = gateId; this.stepNumber = stepNumber; this.lastActionId = lastActionId; this.lastSuccess = lastSuccess;
        }
        static AwaitStep fromWire(Map<String, Object> d) {
            return new AwaitStep(str(d, "gateId"), integer(d, "stepNumber"), strOrNull(d, "lastActionId"), boolOrNull(d, "lastSuccess"));
        }
    }

    /** host → app: the run finished. (Wire fields are lowercase: summary/steps.) */
    public static final class Done {
        public final String summary;
        public final int steps;
        Done(String summary, int steps) { this.summary = summary; this.steps = steps; }
        static Done fromWire(Map<String, Object> d) { return new Done(str(d, "summary"), integer(d, "steps")); }
    }

    /** host → app: inspector / governance telemetry. */
    public static final class LogEntry {
        public final String level, message, stepId;
        LogEntry(String level, String message, String stepId) { this.level = level; this.message = message; this.stepId = stepId; }
        static LogEntry fromWire(Map<String, Object> d) {
            Object lvl = d.get("level");
            return new LogEntry(lvl instanceof String ? (String) lvl : "info", str(d, "message"), strOrNull(d, "stepId"));
        }
    }

    /** One recorded action from the host's durable memory (newest-first). Mirrors kriya::memory::Episode. */
    public static final class Episode {
        public final long tsMs;
        public final String actionId, params, reasoning, signature, runId, goal;
        public final boolean success;
        Episode(long tsMs, String actionId, String params, boolean success, String reasoning, String signature, String runId, String goal) {
            this.tsMs = tsMs; this.actionId = actionId; this.params = params; this.success = success;
            this.reasoning = reasoning; this.signature = signature; this.runId = runId; this.goal = goal;
        }
        static Episode fromWire(Map<String, Object> d) {
            return new Episode(lng(d, "tsMs"), str(d, "actionId"), str(d, "params"), bool(d, "success"),
                str(d, "reasoning"), str(d, "signature"), str(d, "runId"), str(d, "goal"));
        }
    }

    // ── app → host ────────────────────────────────────────────────────────────
    /** app → host: begin an autonomous run. */
    public static final class StartRequest {
        public String goal = "";
        public Map<String, Object> state = new LinkedHashMap<>();
        public List<Map<String, Object>> tools = new ArrayList<>();
        public boolean resume = false;
        public boolean stepMode = false;
        public String agentId;  // nullable
        public String userId;   // nullable

        public StartRequest goal(String g) { this.goal = g; return this; }
        public StartRequest state(Map<String, Object> s) { this.state = s; return this; }
        public StartRequest tools(List<Map<String, Object>> t) { this.tools = t; return this; }
        public StartRequest resume(boolean r) { this.resume = r; return this; }
        public StartRequest stepMode(boolean s) { this.stepMode = s; return this; }
        public StartRequest agentId(String a) { this.agentId = a; return this; }
        public StartRequest userId(String u) { this.userId = u; return this; }

        Map<String, Object> toWire() {
            Map<String, Object> o = new LinkedHashMap<>();
            o.put("goal", goal);
            o.put("state", state);
            o.put("tools", tools);
            o.put("resume", resume);
            o.put("stepMode", stepMode);
            if (agentId != null) o.put("agentId", agentId);
            if (userId != null) o.put("userId", userId);
            return o;
        }
    }

    /** app → host: the result of an action the host asked you to run, plus refreshed state. */
    public static final class ActionResultMsg {
        public String stepId;
        public boolean success;
        public Map<String, Object> state = new LinkedHashMap<>();
        public Object data;   // nullable, must be JSON-able
        public String error;  // nullable

        public ActionResultMsg stepId(String s) { this.stepId = s; return this; }
        public ActionResultMsg success(boolean s) { this.success = s; return this; }
        public ActionResultMsg state(Map<String, Object> s) { this.state = s; return this; }
        public ActionResultMsg data(Object d) { this.data = d; return this; }
        public ActionResultMsg error(String e) { this.error = e; return this; }

        Map<String, Object> toWire() {
            Map<String, Object> o = new LinkedHashMap<>();
            o.put("stepId", stepId);
            o.put("success", success);
            o.put("state", state);
            if (data != null) o.put("data", data);
            if (error != null) o.put("error", error);
            return o;
        }
    }

    /** app → host: a human's decision on a pending approval. */
    public static final class ApprovalResponse {
        public final String stepId;
        public final boolean approved;
        public ApprovalResponse(String stepId, boolean approved) { this.stepId = stepId; this.approved = approved; }
        Map<String, Object> toWire() {
            Map<String, Object> o = new LinkedHashMap<>();
            o.put("stepId", stepId);
            o.put("approved", approved);
            return o;
        }
    }

    /** app → host: advance (true) or stop (false) a step-mode run. */
    public static final class StepAdvance {
        public final String gateId;
        public final boolean proceed;
        public StepAdvance(String gateId, boolean proceed) { this.gateId = gateId; this.proceed = proceed; }
        Map<String, Object> toWire() {
            Map<String, Object> o = new LinkedHashMap<>();
            o.put("gateId", gateId);
            o.put("proceed", proceed);
            return o;
        }
    }
}
