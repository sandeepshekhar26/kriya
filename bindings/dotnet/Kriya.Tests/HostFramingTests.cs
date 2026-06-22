using System.Text.Json.Nodes;
using Kriya;

namespace Kriya.Tests;

public class HostFramingTests
{
    // A Host wired to in-memory streams: a StringWriter captures everything written to stdin, and an
    // empty stdout reader makes the read loop exit immediately so we can drive framing via DispatchLine.
    private static (Host host, StringWriter stdin) Make()
    {
        var stdin = new StringWriter();
        var host = new Host(stdin, new StringReader(""));
        return (host, stdin);
    }

    [Fact]
    public void ParsesActionLineIntoTypedEvent()
    {
        var (host, _) = Make();
        ActionRequest? seen = null;
        host.OnAction += r => seen = r;
        host.DispatchLine("""{"type":"action","data":{"stepId":"s1","actionId":"create_note","params":{"title":"hi"},"reasoning":"r"}}""");
        Assert.NotNull(seen);
        Assert.Equal("create_note", seen!.ActionId);
        Assert.Equal("hi", seen.Params["title"]!.GetValue<string>());
    }

    [Fact]
    public void ParsesDoneLine()
    {
        var (host, _) = Make();
        Done? done = null;
        host.OnDone += d => done = d;
        host.DispatchLine("""{"type":"done","data":{"summary":"all done","steps":2}}""");
        Assert.Equal(2, done!.Steps);
        Assert.Equal("all done", done.Summary);
    }

    [Fact]
    public void UnknownTypeAndBadJsonRaiseParseError()
    {
        var (host, _) = Make();
        var errors = new List<string>();
        host.OnParseError += l => errors.Add(l);
        host.DispatchLine("""{"type":"wat","data":{}}""");
        host.DispatchLine("not json");
        Assert.Equal(2, errors.Count);
    }

    [Fact]
    public void SendActionResultWritesCamelCaseLine()
    {
        var (host, stdin) = Make();
        host.SendActionResult(new ActionResultMsg
        {
            StepId = "s1",
            Success = true,
            State = new JsonObject { ["notes"] = new JsonArray() },
        });
        var msg = JsonNode.Parse(stdin.ToString().Trim())!.AsObject();
        Assert.Equal("action_result", msg["type"]!.GetValue<string>());
        Assert.Equal("s1", msg["data"]!["stepId"]!.GetValue<string>());
        Assert.True(msg["data"]!["success"]!.GetValue<bool>());
    }

    [Fact]
    public void SendApprovalWritesSnakeCaseTypeWithCamelCaseData()
    {
        var (host, stdin) = Make();
        host.SendApproval(new ApprovalResponse { StepId = "s9", Approved = true });
        var msg = JsonNode.Parse(stdin.ToString().Trim())!.AsObject();
        Assert.Equal("approval_response", msg["type"]!.GetValue<string>());
        Assert.Equal("s9", msg["data"]!["stepId"]!.GetValue<string>());
        Assert.True(msg["data"]!["approved"]!.GetValue<bool>());
    }
}
