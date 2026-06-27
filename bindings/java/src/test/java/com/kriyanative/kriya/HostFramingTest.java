package com.kriyanative.kriya;

import com.kriyanative.kriya.Protocol.ActionRequest;
import com.kriyanative.kriya.Protocol.ApprovalRequest;
import com.kriyanative.kriya.Protocol.ApprovalResponse;
import com.kriyanative.kriya.Protocol.AwaitStep;
import com.kriyanative.kriya.Protocol.Done;
import com.kriyanative.kriya.Protocol.Episode;
import com.kriyanative.kriya.Protocol.LogEntry;
import com.kriyanative.kriya.Protocol.StartRequest;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.io.Reader;
import java.io.StringWriter;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;

class HostFramingTest {
    // A stdout that never EOFs — so the reader thread stays alive and doesn't fail pending memory
    // waiters (we drive dispatchLine manually in these unit tests). Daemon thread; harmless at exit.
    private static Reader blockingReader() {
        return new Reader() {
            @Override public int read(char[] cbuf, int off, int len) throws IOException {
                try { Thread.sleep(Long.MAX_VALUE); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                return -1;
            }
            @Override public void close() { }
        };
    }

    private static Host raw(StringWriter out) {
        return new Host(out, blockingReader(), null);
    }

    @Test
    void inboundActionParsed() {
        Host h = raw(new StringWriter());
        AtomicReference<ActionRequest> got = new AtomicReference<>();
        h.onAction(got::set);
        h.dispatchLine("{\"type\":\"action\",\"data\":{\"stepId\":\"s1\",\"actionId\":\"create_note\",\"params\":{\"title\":\"Hi\"},\"reasoning\":\"go\"}}");
        assertNotNull(got.get());
        assertEquals("s1", got.get().stepId);
        assertEquals("create_note", got.get().actionId);
        assertEquals("Hi", got.get().params.get("title"));
        assertEquals("go", got.get().reasoning);
    }

    @Test
    void inboundApprovalParsed() {
        Host h = raw(new StringWriter());
        AtomicReference<ApprovalRequest> got = new AtomicReference<>();
        h.onApproval(got::set);
        h.dispatchLine("{\"type\":\"approval\",\"data\":{\"stepId\":\"s2\",\"actionId\":\"delete_note\",\"params\":{\"id\":3},\"reasoning\":\"risky\"}}");
        assertEquals("delete_note", got.get().actionId);
        assertEquals(3L, ((Number) got.get().params.get("id")).longValue());
    }

    @Test
    void inboundAwaitStepNullableFields() {
        Host h = raw(new StringWriter());
        AtomicReference<AwaitStep> got = new AtomicReference<>();
        h.onAwaitStep(got::set);
        h.dispatchLine("{\"type\":\"await_step\",\"data\":{\"gateId\":\"g1\",\"stepNumber\":2}}");
        assertEquals("g1", got.get().gateId);
        assertEquals(2, got.get().stepNumber);
        assertNull(got.get().lastActionId);
        assertNull(got.get().lastSuccess);
    }

    @Test
    void inboundDoneAndLog() {
        Host h = raw(new StringWriter());
        AtomicReference<Done> done = new AtomicReference<>();
        AtomicReference<LogEntry> log = new AtomicReference<>();
        h.onDone(done::set);
        h.onLog(log::set);
        h.dispatchLine("{\"type\":\"done\",\"data\":{\"summary\":\"ok\",\"steps\":3}}");
        h.dispatchLine("{\"type\":\"log\",\"data\":{\"level\":\"warn\",\"message\":\"held\"}}");
        assertEquals("ok", done.get().summary);
        assertEquals(3, done.get().steps);
        assertEquals("warn", log.get().level);
        assertEquals("held", log.get().message);
    }

    @Test
    void parseErrorsOnBadJsonAndUnknownType() {
        Host h = raw(new StringWriter());
        List<String> errs = new ArrayList<>();
        h.onParseError(errs::add);
        h.dispatchLine("not json");
        h.dispatchLine("{\"type\":\"mystery\",\"data\":{}}");
        assertEquals(2, errs.size());
    }

    @Test
    void outboundStartFrame() {
        StringWriter out = new StringWriter();
        Host h = raw(out);
        h.start(new StartRequest().goal("tidy").agentId("agent-x"));
        Map<String, Object> msg = Json.parseObject(out.toString().trim());
        assertEquals("start", msg.get("type"));
        @SuppressWarnings("unchecked")
        Map<String, Object> data = (Map<String, Object>) msg.get("data");
        assertEquals("tidy", data.get("goal"));
        assertEquals("agent-x", data.get("agentId"));
        assertEquals(Boolean.FALSE, data.get("resume"));
        assertFalse(data.containsKey("userId")); // null omitted
    }

    @Test
    void outboundApprovalFrame() {
        StringWriter out = new StringWriter();
        Host h = raw(out);
        h.sendApproval(new ApprovalResponse("s9", true));
        Map<String, Object> msg = Json.parseObject(out.toString().trim());
        assertEquals("approval_response", msg.get("type"));
        @SuppressWarnings("unchecked")
        Map<String, Object> data = (Map<String, Object>) msg.get("data");
        assertEquals("s9", data.get("stepId"));
        assertEquals(Boolean.TRUE, data.get("approved"));
    }

    @Test
    void recentMemoryRoundTrip() throws Exception {
        StringWriter out = new StringWriter();
        Host h = raw(out);
        CompletableFuture<List<Episode>> fut = h.recentMemory(5);

        // the request was framed with requestId "mem-1" (first call)
        Map<String, Object> req = Json.parseObject(out.toString().trim());
        @SuppressWarnings("unchecked")
        Map<String, Object> data = (Map<String, Object>) req.get("data");
        assertEquals("memory_recent", req.get("type"));
        String reqId = (String) data.get("requestId");
        assertEquals(5L, ((Number) data.get("limit")).longValue());

        // simulate the host's reply
        h.dispatchLine("{\"type\":\"memory\",\"data\":{\"requestId\":\"" + reqId + "\",\"episodes\":[" +
            "{\"tsMs\":1700000000000,\"actionId\":\"create_note\",\"params\":\"{}\",\"success\":true,\"signature\":\"abcdef0123\",\"runId\":\"r1\",\"goal\":\"g\"}]}}");

        List<Episode> eps = fut.get(2, TimeUnit.SECONDS);
        assertEquals(1, eps.size());
        assertEquals("create_note", eps.get(0).actionId);
        assertTrue(eps.get(0).success);
        assertEquals(1700000000000L, eps.get(0).tsMs);
    }

    @Test
    void recentMemoryTimesOut() {
        Host h = raw(new StringWriter());
        CompletableFuture<List<Episode>> fut = h.recentMemory(1, Duration.ofMillis(50));
        assertThrows(ExecutionException.class, () -> fut.get(2, TimeUnit.SECONDS));
    }
}
