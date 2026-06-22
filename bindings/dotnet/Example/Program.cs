using System.Text.Json.Nodes;
using Kriya;

// kriya .NET example — host the governed agent runtime from a plain .NET console app. This is the
// macOS-runnable parallel of examples/node-sidecar-host: your app spawns the kriya-host binary and
// drives a governed run; the agent loop, the policy engine, human approval, the budget, and the
// signed audit log all live inside that separate process — which your UI can't tamper with. Your
// process only ever runs the typed actions the host has already cleared. Every line here is the same
// shape a WPF / WinForms / Avalonia main process would run.
//
//   KRIYA_HOST_BIN=.../kriya-host dotnet run --project bindings/dotnet/Example

var binaryPath = Environment.GetEnvironmentVariable("KRIYA_HOST_BIN")
    ?? Path.GetFullPath(Path.Combine(AppContext.BaseDirectory,
        "..", "..", "..", "..", "..", "..", "apps", "note-app", "src-tauri", "target", "debug", "kriya-host"));

if (!File.Exists(binaryPath))
{
    Console.Error.WriteLine($"\nkriya-host not found at:\n  {binaryPath}\n\nBuild it first:\n" +
        "  (cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)\n" +
        "or set KRIYA_HOST_BIN to your built binary.\n");
    return 1;
}

// The app's own state + typed actions — the SAME handlers a human button would call. The agent never
// touches this directly; it proposes an action id + params, and the governed host decides whether
// that proposal is allowed to reach these handlers.
var notes = new List<JsonObject>();
var nextId = 1;
var reg = new Registry();

reg.RegisterAction("create_note", "Create a note with a title.",
    (p, _) =>
    {
        var id = nextId++;
        notes.Add(new JsonObject { ["id"] = id, ["title"] = p["title"]?.GetValue<string>() });
        Console.WriteLine($"  [run]      create_note({p.ToJsonString()})");
        return ActionResult.Ok(new JsonObject { ["id"] = id });
    },
    parameters: new() { ["title"] = P.Required(P.Str) });

reg.RegisterAction("delete_note", "Delete a note by id.",
    (p, _) =>
    {
        var id = p["id"]!.GetValue<int>();
        notes.RemoveAll(n => n["id"]!.GetValue<int>() == id);
        Console.WriteLine($"  [run]      delete_note({p.ToJsonString()})");
        return ActionResult.Ok(new JsonObject { ["deleted"] = id });
    },
    parameters: new() { ["id"] = P.Required(P.Num) });

// A deterministic, no-API-key script for the demo (the host's ScriptedPlanner). Set
// AGENT_BACKEND=claude-cli|ollama|anthropic on the spawn env to drive it with a real model instead.
var script = Path.Combine(Path.GetTempPath(), $"kriya-dotnet-demo-{Guid.NewGuid():N}.json");
File.WriteAllText(script,
    """[{"action":"create_note","params":{"title":"Groceries"},"reasoning":"seed a note to work with"},{"action":"create_note","params":{"title":"scratch — delete me"},"reasoning":"a throwaway note"},{"action":"delete_note","params":{"id":2},"reasoning":"remove the throwaway — this one needs a human"},{"done":true,"summary":"created two notes and removed the throwaway"}]""");

using var host = Host.Spawn(binaryPath, new[] { "--script", script });
Console.WriteLine("\n=== hosting kriya-host from .NET — governance runs in the child process ===\n");

var state = new JsonObject { ["notes"] = new JsonArray() };
var done = await Host.RunTask(host, reg, goal: "tidy up the notes", state: state,
    // A policy-guarded action (delete_*) pauses the in-flight run HERE for a human. We grant it.
    // In a real app: forward this to a modal in your UI and return the human's answer.
    approve: req =>
    {
        Console.WriteLine($"  [APPROVE]  \"{req.ActionId}\" needs a human — granting (reason: {req.Reasoning})");
        return true;
    },
    onLog: e => Console.WriteLine($"             - [{e.Level}] {e.Message}"));

Console.WriteLine($"\n=== done: \"{done.Summary}\" ({done.Steps} step(s)) ===");
Console.WriteLine($"    final notes: [{string.Join(", ", notes.Select(n => n["title"]?.GetValue<string>()))}]\n");

// Durable signed memory over the same protocol — the same episodic log Tauri reads.
var episodes = await host.RecentMemoryAsync(5);
Console.WriteLine($"=== RecentMemory(): {episodes.Count} newest episode(s) (persists across runs) ===");
foreach (var ep in episodes)
{
    var when = DateTimeOffset.FromUnixTimeMilliseconds(ep.TsMs).ToString("u");
    var sig = ep.Signature.Length > 12 ? ep.Signature[..12] : ep.Signature;
    Console.WriteLine($"    {when}  {ep.ActionId,-12} {(ep.Success ? "ok " : "err")}  sig={sig}…");
}
Console.WriteLine();

host.Close();
File.Delete(script);
return 0;
