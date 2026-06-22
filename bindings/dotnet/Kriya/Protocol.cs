using System.Text.Json.Nodes;

namespace Kriya;

// The agent-loop wire protocol the .NET binding speaks to kriya-host over stdio. Each line is a JSON
// object {"type": ..., "data": {...}}. `data` fields are camelCase (the Rust structs use
// rename_all="camelCase"), except `done` whose fields are already lowercase. These types use
// idiomatic PascalCase and convert at the edge via FromWire / ToWire — mirrors protocol.ts / the
// Python binding.

internal static class W
{
    public static string Str(JsonObject d, string k) => d[k]?.GetValue<string>() ?? "";
    public static string? StrOrNull(JsonObject d, string k) => d[k]?.GetValue<string>();
    public static JsonObject Obj(JsonObject d, string k) => d[k] as JsonObject ?? new JsonObject();
    public static bool Bool(JsonObject d, string k) => d[k]?.GetValue<bool>() ?? false;
    public static bool? BoolOrNull(JsonObject d, string k) => d[k] is JsonValue v && v.TryGetValue<bool>(out var b) ? b : null;
    public static long Long(JsonObject d, string k) => d[k]?.GetValue<long>() ?? 0;
    public static int Int(JsonObject d, string k) => d[k]?.GetValue<int>() ?? 0;
}

// ── host → app ──────────────────────────────────────────────────────────────

/// <summary>host → app: run this action and reply with an <see cref="ActionResultMsg"/> (same StepId).</summary>
public sealed class ActionRequest
{
    public string StepId { get; init; } = "";
    public string ActionId { get; init; } = "";
    public JsonObject Params { get; init; } = new();
    public string Reasoning { get; init; } = "";

    public static ActionRequest FromWire(JsonObject d) => new()
    {
        StepId = W.Str(d, "stepId"),
        ActionId = W.Str(d, "actionId"),
        Params = W.Obj(d, "params"),
        Reasoning = W.Str(d, "reasoning"),
    };
}

/// <summary>host → app: a guarded action needs a human's go-ahead. Reply with <see cref="ApprovalResponse"/>.</summary>
public sealed class ApprovalRequest
{
    public string StepId { get; init; } = "";
    public string ActionId { get; init; } = "";
    public JsonObject Params { get; init; } = new();
    public string Reasoning { get; init; } = "";

    public static ApprovalRequest FromWire(JsonObject d) => new()
    {
        StepId = W.Str(d, "stepId"),
        ActionId = W.Str(d, "actionId"),
        Params = W.Obj(d, "params"),
        Reasoning = W.Str(d, "reasoning"),
    };
}

/// <summary>host → app: paused before a step (step-mode). Reply with <see cref="StepAdvance"/>.</summary>
public sealed class AwaitStep
{
    public string GateId { get; init; } = "";
    public int StepNumber { get; init; }
    public string? LastActionId { get; init; }
    public bool? LastSuccess { get; init; }

    public static AwaitStep FromWire(JsonObject d) => new()
    {
        GateId = W.Str(d, "gateId"),
        StepNumber = W.Int(d, "stepNumber"),
        LastActionId = W.StrOrNull(d, "lastActionId"),
        LastSuccess = W.BoolOrNull(d, "lastSuccess"),
    };
}

/// <summary>host → app: the run finished. (Wire fields are lowercase: summary/steps.)</summary>
public sealed class Done
{
    public string Summary { get; init; } = "";
    public int Steps { get; init; }

    public static Done FromWire(JsonObject d) => new() { Summary = W.Str(d, "summary"), Steps = W.Int(d, "steps") };
}

/// <summary>host → app: inspector/governance telemetry.</summary>
public sealed class LogEntry
{
    public string Level { get; init; } = "info";
    public string Message { get; init; } = "";
    public string? StepId { get; init; }

    public static LogEntry FromWire(JsonObject d) => new()
    {
        Level = d["level"]?.GetValue<string>() ?? "info",
        Message = W.Str(d, "message"),
        StepId = W.StrOrNull(d, "stepId"),
    };
}

/// <summary>One recorded action from the host's durable memory (newest-first). Params is the
/// JSON-encoded string exactly as signed. Mirrors kriya::memory::Episode.</summary>
public sealed class Episode
{
    public long TsMs { get; init; }
    public string ActionId { get; init; } = "";
    public string Params { get; init; } = "";
    public bool Success { get; init; }
    public string Reasoning { get; init; } = "";
    public string Signature { get; init; } = "";
    public string RunId { get; init; } = "";
    public string Goal { get; init; } = "";

    public static Episode FromWire(JsonObject d) => new()
    {
        TsMs = W.Long(d, "tsMs"),
        ActionId = W.Str(d, "actionId"),
        Params = W.Str(d, "params"),
        Success = W.Bool(d, "success"),
        Reasoning = W.Str(d, "reasoning"),
        Signature = W.Str(d, "signature"),
        RunId = W.Str(d, "runId"),
        Goal = W.Str(d, "goal"),
    };
}

// ── app → host ──────────────────────────────────────────────────────────────

/// <summary>app → host: begin an autonomous run.</summary>
public sealed class StartRequest
{
    public required string Goal { get; init; }
    public JsonObject State { get; init; } = new();
    public IReadOnlyList<JsonObject> Tools { get; init; } = System.Array.Empty<JsonObject>();
    public bool Resume { get; init; }
    public bool StepMode { get; init; }
    public string? AgentId { get; init; }
    public string? UserId { get; init; }

    public JsonObject ToWire()
    {
        var tools = new JsonArray();
        foreach (var t in Tools) tools.Add(t.DeepClone());
        var o = new JsonObject
        {
            ["goal"] = Goal,
            ["state"] = State.DeepClone(),
            ["tools"] = tools,
            ["resume"] = Resume,
            ["stepMode"] = StepMode,
        };
        if (AgentId is not null) o["agentId"] = AgentId;
        if (UserId is not null) o["userId"] = UserId;
        return o;
    }
}

/// <summary>app → host: the result of an action the host asked you to run, plus refreshed state.</summary>
public sealed class ActionResultMsg
{
    public required string StepId { get; init; }
    public required bool Success { get; init; }
    public JsonObject State { get; init; } = new();
    public object? Data { get; init; }
    public string? Error { get; init; }

    public JsonObject ToWire()
    {
        var o = new JsonObject
        {
            ["stepId"] = StepId,
            ["success"] = Success,
            ["state"] = State.DeepClone(),
        };
        if (Data is not null) o["data"] = System.Text.Json.JsonSerializer.SerializeToNode(Data);
        if (Error is not null) o["error"] = Error;
        return o;
    }
}

/// <summary>app → host: a human's decision on a pending approval.</summary>
public sealed class ApprovalResponse
{
    public required string StepId { get; init; }
    public required bool Approved { get; init; }
    public JsonObject ToWire() => new() { ["stepId"] = StepId, ["approved"] = Approved };
}

/// <summary>app → host: advance (true) or stop (false) a step-mode run.</summary>
public sealed class StepAdvance
{
    public required string GateId { get; init; }
    public required bool Proceed { get; init; }
    public JsonObject ToWire() => new() { ["gateId"] = GateId, ["proceed"] = Proceed };
}
