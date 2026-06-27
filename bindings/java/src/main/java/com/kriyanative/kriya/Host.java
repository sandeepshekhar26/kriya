package com.kriyanative.kriya;

import com.kriyanative.kriya.Protocol.ActionRequest;
import com.kriyanative.kriya.Protocol.ActionResultMsg;
import com.kriyanative.kriya.Protocol.ApprovalRequest;
import com.kriyanative.kriya.Protocol.ApprovalResponse;
import com.kriyanative.kriya.Protocol.AwaitStep;
import com.kriyanative.kriya.Protocol.Done;
import com.kriyanative.kriya.Protocol.Episode;
import com.kriyanative.kriya.Protocol.LogEntry;
import com.kriyanative.kriya.Protocol.StartRequest;
import com.kriyanative.kriya.Protocol.StepAdvance;

import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.io.Reader;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.Predicate;

/**
 * A live connection to a {@code kriya-host} sidecar — the Rust agent host. Spawns the binary (or wraps
 * raw streams for tests) and speaks its newline-delimited JSON protocol over stdio. The agent loop,
 * the inference backend, and the whole safety layer (policy, approval, budget, signed audit) run inside
 * that separate process — which your app/UI can't tamper with — while your JVM process only runs the
 * typed actions the host has already cleared. The cross-shell binding of kriya for the JVM.
 */
public final class Host implements AutoCloseable {
    private final Writer stdin;
    private final Process proc;          // nullable (raw-stream constructor for tests)
    private final Object writeLock = new Object();
    private final ConcurrentHashMap<String, CompletableFuture<List<Episode>>> memoryWaiters = new ConcurrentHashMap<>();
    private final AtomicInteger memorySeq = new AtomicInteger();
    private volatile boolean closed;

    private final List<Consumer<ActionRequest>> onAction = new CopyOnWriteArrayList<>();
    private final List<Consumer<ApprovalRequest>> onApproval = new CopyOnWriteArrayList<>();
    private final List<Consumer<AwaitStep>> onAwaitStep = new CopyOnWriteArrayList<>();
    private final List<Consumer<Done>> onDone = new CopyOnWriteArrayList<>();
    private final List<Consumer<LogEntry>> onLog = new CopyOnWriteArrayList<>();
    private final List<Consumer<String>> onParseError = new CopyOnWriteArrayList<>();
    private final List<Consumer<Integer>> onExit = new CopyOnWriteArrayList<>();

    /** Construct from raw streams (handy for tests). Usually use {@link #spawn}. */
    public Host(Writer stdin, Reader stdout, Process proc) {
        this.stdin = stdin;
        this.proc = proc;
        Thread reader = new Thread(() -> readLoop(stdout), "kriya-host-reader");
        reader.setDaemon(true);
        reader.start();
        if (proc != null) {
            proc.onExit().thenAccept(p -> {
                failPendingMemory("kriya-host exited before replying");
                Integer code = null;
                try { code = p.exitValue(); } catch (IllegalThreadStateException ignored) { }
                fire(onExit, code);
            });
        }
    }

    /** Spawn the {@code kriya-host} binary and connect to it. stderr is inherited so the host's banner
     * and governance log show up in your console. */
    public static Host spawn(String binaryPath, List<String> args, Map<String, String> env) {
        List<String> cmd = new ArrayList<>();
        cmd.add(binaryPath);
        if (args != null) cmd.addAll(args);
        ProcessBuilder pb = new ProcessBuilder(cmd);
        pb.redirectError(ProcessBuilder.Redirect.INHERIT);
        if (env != null) pb.environment().putAll(env);
        Process proc;
        try {
            proc = pb.start();
        } catch (IOException e) {
            throw new RuntimeException("failed to start kriya-host: " + e.getMessage(), e);
        }
        Writer w = new BufferedWriter(new OutputStreamWriter(proc.getOutputStream(), StandardCharsets.UTF_8));
        Reader r = new BufferedReader(new InputStreamReader(proc.getInputStream(), StandardCharsets.UTF_8));
        return new Host(w, r, proc);
    }

    public static Host spawn(String binaryPath, List<String> args) { return spawn(binaryPath, args, null); }

    // ── event subscription (append-style, like the .NET events) ───────────────
    public void onAction(Consumer<ActionRequest> c) { onAction.add(c); }
    public void onApproval(Consumer<ApprovalRequest> c) { onApproval.add(c); }
    public void onAwaitStep(Consumer<AwaitStep> c) { onAwaitStep.add(c); }
    public void onDone(Consumer<Done> c) { onDone.add(c); }
    public void onLog(Consumer<LogEntry> c) { onLog.add(c); }
    public void onParseError(Consumer<String> c) { onParseError.add(c); }
    public void onExit(Consumer<Integer> c) { onExit.add(c); }

    private static <T> void fire(List<Consumer<T>> handlers, T arg) {
        for (Consumer<T> h : handlers) h.accept(arg);
    }

    private void readLoop(Reader stdout) {
        try (BufferedReader br = stdout instanceof BufferedReader ? (BufferedReader) stdout : new BufferedReader(stdout)) {
            String line;
            while ((line = br.readLine()) != null) {
                String trimmed = line.trim();
                if (!trimmed.isEmpty()) dispatchLine(trimmed);
            }
        } catch (IOException ignored) {
            /* stream closed */
        }
        failPendingMemory("kriya-host stdout closed");
    }

    /** Parse + route one outbound (host→app) line. Package-private so tests can drive framing directly. */
    void dispatchLine(String line) {
        Map<String, Object> msg;
        try {
            msg = Json.parseObject(line);
        } catch (RuntimeException e) {
            fire(onParseError, line);
            return;
        }
        Object typeObj = msg.get("type");
        String type = typeObj instanceof String ? (String) typeObj : null;
        @SuppressWarnings("unchecked")
        Map<String, Object> data = msg.get("data") instanceof Map ? (Map<String, Object>) msg.get("data") : new LinkedHashMap<>();
        if (type == null) { fire(onParseError, line); return; }
        switch (type) {
            case "action": fire(onAction, ActionRequest.fromWire(data)); break;
            case "approval": fire(onApproval, ApprovalRequest.fromWire(data)); break;
            case "await_step": fire(onAwaitStep, AwaitStep.fromWire(data)); break;
            case "done": fire(onDone, Done.fromWire(data)); break;
            case "log": fire(onLog, LogEntry.fromWire(data)); break;
            case "memory": {
                Object reqIdObj = data.get("requestId");
                String reqId = reqIdObj instanceof String ? (String) reqIdObj : null;
                List<Episode> episodes = new ArrayList<>();
                Object eps = data.get("episodes");
                if (eps instanceof List) {
                    for (Object e : (List<?>) eps) {
                        if (e instanceof Map) {
                            @SuppressWarnings("unchecked")
                            Map<String, Object> eo = (Map<String, Object>) e;
                            episodes.add(Episode.fromWire(eo));
                        }
                    }
                }
                CompletableFuture<List<Episode>> waiter = reqId == null ? null : memoryWaiters.remove(reqId);
                if (waiter != null) waiter.complete(episodes);
                else fire(onParseError, line);
                break;
            }
            default: fire(onParseError, line); break;
        }
    }

    private void send(String type, Map<String, Object> data) {
        Map<String, Object> msg = new LinkedHashMap<>();
        msg.put("type", type);
        msg.put("data", data);
        String s = Json.stringify(msg);
        synchronized (writeLock) {
            try {
                stdin.write(s);
                stdin.write('\n');
                stdin.flush();
            } catch (IOException e) {
                throw new RuntimeException("failed to write to kriya-host: " + e.getMessage(), e);
            }
        }
    }

    /** Begin an autonomous run. */
    public void start(StartRequest req) { send("start", req.toWire()); }
    /** Report the result of an action the host asked you to run. */
    public void sendActionResult(ActionResultMsg result) { send("action_result", result.toWire()); }
    /** Answer a pending approval request. */
    public void sendApproval(ApprovalResponse response) { send("approval_response", response.toWire()); }
    /** Advance or stop a step-mode run. */
    public void sendStepAdvance(StepAdvance advance) { send("step_advance", advance.toWire()); }

    /** Read the newest episodes from the host's durable memory — the JVM equivalent of the Tauri
     * {@code agent_memory_recent} command. Newest first; the future fails on timeout or host exit. */
    public CompletableFuture<List<Episode>> recentMemory(Integer limit, Duration timeout) {
        String requestId = "mem-" + memorySeq.incrementAndGet();
        CompletableFuture<List<Episode>> fut = new CompletableFuture<>();
        memoryWaiters.put(requestId, fut);

        Map<String, Object> data = new LinkedHashMap<>();
        data.put("requestId", requestId);
        if (limit != null) data.put("limit", limit);
        send("memory_recent", data);

        long ms = (timeout != null ? timeout : Duration.ofSeconds(5)).toMillis();
        return fut.orTimeout(ms, TimeUnit.MILLISECONDS)
            .whenComplete((r, e) -> memoryWaiters.remove(requestId));
    }

    public CompletableFuture<List<Episode>> recentMemory(Integer limit) { return recentMemory(limit, null); }

    private void failPendingMemory(String reason) {
        for (String key : new ArrayList<>(memoryWaiters.keySet())) {
            CompletableFuture<List<Episode>> w = memoryWaiters.remove(key);
            if (w != null) w.completeExceptionally(new IOException(reason));
        }
    }

    /** Close stdin (signals shutdown) and kill the child if we own it. */
    @Override
    public void close() {
        if (closed) return;
        closed = true;
        failPendingMemory("Host closed");
        try { stdin.close(); } catch (IOException ignored) { }
        if (proc != null && proc.isAlive()) proc.destroy();
    }

    // ── high-level runTask ────────────────────────────────────────────────────

    /** Drive a single run to completion: send start, execute each action via {@code dispatch}, answer
     * approvals, gate step-mode, and resolve with the {@link Done} summary. */
    public static CompletableFuture<Done> runTask(Host host, StartRequest req,
            Function<ActionRequest, ActionResultMsg> dispatch,
            Predicate<ApprovalRequest> approve,
            Predicate<AwaitStep> onStep,
            Consumer<LogEntry> onLog) {
        CompletableFuture<Done> fut = new CompletableFuture<>();
        host.onAction(r -> host.sendActionResult(dispatch.apply(r)));
        host.onApproval(r -> host.sendApproval(new ApprovalResponse(r.stepId, approve != null && approve.test(r))));
        host.onAwaitStep(ev -> host.sendStepAdvance(new StepAdvance(ev.gateId, onStep == null || onStep.test(ev))));
        if (onLog != null) host.onLog(onLog);
        host.onDone(fut::complete);
        host.start(req);
        return fut;
    }

    /** Registry-driven run: tools come from the {@code registry} and each action is dispatched to its
     * handler. {@code state} is sent back each step (mutate it in your handlers). */
    public static CompletableFuture<Done> runTask(Host host, Registry registry, String goal, Map<String, Object> state,
            Predicate<ApprovalRequest> approve, Predicate<AwaitStep> onStep, Consumer<LogEntry> onLog,
            boolean resume, boolean stepMode, String agentId, String userId) {
        StartRequest req = new StartRequest()
            .goal(goal).state(state).tools(registry.toolSchemas())
            .resume(resume).stepMode(stepMode).agentId(agentId).userId(userId);
        return runTask(host, req,
            ar -> {
                ActionResult result = registry.dispatchAction(ar.actionId, ar.params, ActionContext.agent(ar.stepId));
                return new ActionResultMsg().stepId(ar.stepId).success(result.success).data(result.data).error(result.error).state(state);
            },
            approve, onStep, onLog);
    }

    /** Registry-driven run with sensible defaults (no step-mode, no resume, no actor). */
    public static CompletableFuture<Done> runTask(Host host, Registry registry, String goal, Map<String, Object> state,
            Predicate<ApprovalRequest> approve, Consumer<LogEntry> onLog) {
        return runTask(host, registry, goal, state, approve, null, onLog, false, false, null, null);
    }
}
