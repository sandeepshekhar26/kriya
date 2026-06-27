package com.kriyanative.kriya.example;

import com.kriyanative.kriya.ActionResult;
import com.kriyanative.kriya.Host;
import com.kriyanative.kriya.P;
import com.kriyanative.kriya.ParameterSchema;
import com.kriyanative.kriya.Protocol.Done;
import com.kriyanative.kriya.Protocol.Episode;
import com.kriyanative.kriya.Registry;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * kriya JVM example — host the governed agent runtime from a plain Java program. The JVM-runnable
 * parallel of examples/node-sidecar-host: your app spawns the kriya-host binary and drives a governed
 * run; the agent loop, the policy engine, human approval, the budget, and the signed audit log all
 * live inside that separate process — which your UI can't tamper with. Your process only ever runs the
 * typed actions the host has already cleared. Every line here is the same shape a Swing / JavaFX main
 * would run.
 *
 *   KRIYA_HOST_BIN=.../kriya-host  mvn -q -pl bindings/java compile exec:java
 */
public final class NoteAppHost {
    public static void main(String[] args) throws Exception {
        String binaryPath = resolveHostBin();
        if (binaryPath == null) {
            System.err.println("\nkriya-host not found. Build it first:\n" +
                "  cargo build -p kriya --bin kriya-host --features sidecar-host\n" +
                "or set KRIYA_HOST_BIN to your built binary.\n");
            System.exit(1);
            return;
        }

        // The app's own state + typed actions — the SAME handlers a human button would call. The agent
        // never touches this directly; it proposes an action id + params, and the governed host decides
        // whether that proposal reaches these handlers.
        final List<Map<String, Object>> notes = new ArrayList<>();
        final int[] nextId = { 1 };
        Registry reg = new Registry();

        Map<String, ParameterSchema> createParams = P.params();
        createParams.put("title", P.required(P.str()));
        reg.registerAction("create_note", "Create a note with a title.", (p, ctx) -> {
            int id = nextId[0]++;
            Map<String, Object> note = new LinkedHashMap<>();
            note.put("id", id);
            note.put("title", p.get("title"));
            notes.add(note);
            System.out.println("  [run]      create_note(" + p + ")");
            Map<String, Object> data = new LinkedHashMap<>();
            data.put("id", id);
            return ActionResult.ok(data);
        }, createParams);

        Map<String, ParameterSchema> deleteParams = P.params();
        deleteParams.put("id", P.required(P.num()));
        reg.registerAction("delete_note", "Delete a note by id.", (p, ctx) -> {
            long id = ((Number) p.get("id")).longValue();
            notes.removeIf(n -> ((Number) n.get("id")).longValue() == id);
            System.out.println("  [run]      delete_note(" + p + ")");
            Map<String, Object> data = new LinkedHashMap<>();
            data.put("deleted", id);
            return ActionResult.ok(data);
        }, deleteParams);

        // A deterministic, no-API-key script for the demo (the host's ScriptedPlanner).
        Path script = Files.createTempFile("kriya-java-demo-", ".json");
        Files.write(script, ("[{\"action\":\"create_note\",\"params\":{\"title\":\"Groceries\"},\"reasoning\":\"seed a note\"}," +
            "{\"action\":\"create_note\",\"params\":{\"title\":\"scratch — delete me\"},\"reasoning\":\"a throwaway note\"}," +
            "{\"action\":\"delete_note\",\"params\":{\"id\":2},\"reasoning\":\"remove the throwaway — needs a human\"}," +
            "{\"done\":true,\"summary\":\"created two notes and removed the throwaway\"}]").getBytes(StandardCharsets.UTF_8));

        try (Host host = Host.spawn(binaryPath, Arrays.asList("--script", script.toString()))) {
            System.out.println("\n=== hosting kriya-host from the JVM — governance runs in the child process ===\n");

            Map<String, Object> state = new LinkedHashMap<>();
            state.put("notes", new ArrayList<>());

            Done done = Host.runTask(host, reg, "tidy up the notes", state,
                // A policy-guarded action (delete_*) pauses the in-flight run HERE for a human. We grant
                // it. In a real app: forward this to a modal in your UI and return the human's answer.
                req -> {
                    System.out.println("  [APPROVE]  \"" + req.actionId + "\" needs a human — granting (reason: " + req.reasoning + ")");
                    return true;
                },
                e -> System.out.println("             - [" + e.level + "] " + e.message)
            ).get();

            List<String> titles = new ArrayList<>();
            for (Map<String, Object> n : notes) titles.add(String.valueOf(n.get("title")));
            System.out.println("\n=== done: \"" + done.summary + "\" (" + done.steps + " step(s)) ===");
            System.out.println("    final notes: [" + String.join(", ", titles) + "]\n");

            // Durable signed memory over the same protocol — the same episodic log Tauri reads.
            List<Episode> episodes = host.recentMemory(5).get();
            System.out.println("=== recentMemory(): " + episodes.size() + " newest episode(s) (persists across runs) ===");
            for (Episode ep : episodes) {
                String when = Instant.ofEpochMilli(ep.tsMs).toString();
                String sig = ep.signature.length() > 12 ? ep.signature.substring(0, 12) : ep.signature;
                System.out.printf("    %s  %-12s %s  sig=%s…%n", when, ep.actionId, ep.success ? "ok " : "err", sig);
            }
            System.out.println();
        } finally {
            Files.deleteIfExists(script);
        }
    }

    /** KRIYA_HOST_BIN, else a few candidate build paths relative to the repo root. */
    private static String resolveHostBin() {
        String env = System.getenv("KRIYA_HOST_BIN");
        if (env != null && !env.isEmpty() && new File(env).isFile()) return env;
        File dir = new File("").getAbsoluteFile();
        for (int i = 0; i < 6 && dir != null; i++) {
            if (new File(dir, "crates").isDirectory()) {
                for (String rel : new String[] {
                        "crates/kriya/target/debug/kriya-host",
                        "target/debug/kriya-host",
                        "apps/note-app/src-tauri/target/debug/kriya-host" }) {
                    File cand = new File(dir, rel);
                    if (cand.isFile()) return cand.getAbsolutePath();
                }
            }
            dir = dir.getParentFile();
        }
        return null;
    }

    private NoteAppHost() {}
}
