package com.kriyanative.kriya;

import com.kriyanative.kriya.Protocol.ActionResultMsg;
import com.kriyanative.kriya.Protocol.Done;
import com.kriyanative.kriya.Protocol.Episode;
import com.kriyanative.kriya.Protocol.StartRequest;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Drives the REAL kriya-host binary end to end. Opt-in: set KRIYA_HOST_BIN to the built path
 * (e.g. crates/kriya/target/debug/kriya-host). Skipped when unset so the unit suite stays
 * self-contained — mirrors the Node/Python/.NET integration tests.
 */
class IntegrationTest {
    private static String hostBin() {
        String bin = System.getenv("KRIYA_HOST_BIN");
        return (bin != null && !bin.isEmpty() && new File(bin).isFile()) ? bin : null;
    }

    @Test
    void drivesScriptedRunThroughApprovalAndMemory() throws Exception {
        String bin = hostBin();
        if (bin == null) return; // opt-in — set KRIYA_HOST_BIN to run

        Path dir = Files.createTempDirectory("kriya-java-itest");
        try {
            Path script = dir.resolve("script.json");
            Files.write(script, ("[{\"action\":\"create_note\",\"params\":{\"title\":\"From Java\"},\"reasoning\":\"seed\"}," +
                "{\"action\":\"delete_note\",\"params\":{\"id\":1},\"reasoning\":\"cleanup\"}," +
                "{\"done\":true,\"summary\":\"done\"}]").getBytes(StandardCharsets.UTF_8));

            try (Host host = Host.spawn(bin, Arrays.asList("--script", script.toString()))) {
                String goal = "java-itest-" + UUID.randomUUID().toString().replace("-", "");
                List<String> approvals = new CopyOnWriteArrayList<>();

                Map<String, Object> state = new LinkedHashMap<>();
                state.put("notes", new ArrayList<>());

                Done done = Host.runTask(host,
                    new StartRequest().goal(goal).state(state).tools(new ArrayList<>()),
                    ar -> new ActionResultMsg().stepId(ar.stepId).success(true).state(new LinkedHashMap<>()),
                    ap -> { approvals.add(ap.actionId); return true; },
                    null, null
                ).get(30, TimeUnit.SECONDS);

                assertEquals(2, done.steps);
                // delete_* requires approval under the host's default policy; create_* is allowed.
                assertTrue(approvals.contains("delete_note"), "expected delete_note to require approval");

                List<String> mine = new ArrayList<>();
                for (Episode e : host.recentMemory(50).get(10, TimeUnit.SECONDS)) {
                    if (goal.equals(e.goal)) mine.add(e.actionId);
                }
                assertTrue(mine.contains("create_note"), "create_note should be in memory");
                assertTrue(mine.contains("delete_note"), "delete_note should be in memory");
            }
        } finally {
            deleteRecursively(dir.toFile());
        }
    }

    private static void deleteRecursively(File f) {
        File[] kids = f.listFiles();
        if (kids != null) for (File k : kids) deleteRecursively(k);
        //noinspection ResultOfMethodCallIgnored
        f.delete();
    }
}
