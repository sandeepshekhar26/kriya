using System.Text.Json.Nodes;
using Kriya;

namespace Kriya.Tests;

/// <summary>
/// Drives the REAL kriya-host binary end to end. Opt-in: set KRIYA_HOST_BIN to the built path
/// (e.g. apps/note-app/src-tauri/target/debug/kriya-host). Skipped when unset so the unit suite stays
/// self-contained — mirrors the Node/Python integration tests.
/// </summary>
public class IntegrationTests
{
    private static string? HostBin()
    {
        var bin = Environment.GetEnvironmentVariable("KRIYA_HOST_BIN");
        return !string.IsNullOrEmpty(bin) && File.Exists(bin) ? bin : null;
    }

    [Fact]
    public async Task DrivesScriptedRunThroughApprovalAndMemory()
    {
        var bin = HostBin();
        if (bin is null) return; // opt-in — set KRIYA_HOST_BIN to run

        var dir = Directory.CreateTempSubdirectory("kriya-dotnet-itest");
        try
        {
            var script = Path.Combine(dir.FullName, "script.json");
            await File.WriteAllTextAsync(script,
                """[{"action":"create_note","params":{"title":"From .NET"},"reasoning":"seed"},{"action":"delete_note","params":{"id":1},"reasoning":"cleanup"},{"done":true,"summary":"done"}]""");

            using var host = Host.Spawn(bin, new[] { "--script", script });
            var goal = $"dotnet-itest-{Guid.NewGuid():N}";
            var approvals = new List<string>();

            var done = await Host.RunTask(host,
                new StartRequest
                {
                    Goal = goal,
                    State = new JsonObject { ["notes"] = new JsonArray() },
                    Tools = System.Array.Empty<JsonObject>(),
                },
                dispatch: ar => new ActionResultMsg
                {
                    StepId = ar.StepId,
                    Success = true,
                    State = new JsonObject { ["notes"] = new JsonArray() },
                },
                approve: ap => { approvals.Add(ap.ActionId); return true; });

            Assert.Equal(2, done.Steps);
            // delete_* requires approval under the host's default policy; create_* is allowed.
            Assert.Contains("delete_note", approvals);

            var mine = (await host.RecentMemoryAsync(50))
                .Where(e => e.Goal == goal)
                .Select(e => e.ActionId)
                .ToList();
            Assert.Contains("create_note", mine);
            Assert.Contains("delete_note", mine);
        }
        finally
        {
            dir.Delete(recursive: true);
        }
    }
}
