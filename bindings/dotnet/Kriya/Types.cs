using System.Text.Json.Nodes;

namespace Kriya;

/// <summary>The subset of JSON Schema kriya uses to describe one action parameter.</summary>
public enum ParamType { String, Number, Boolean, Array, Object }

/// <summary>
/// The schema for a single action parameter. Immutable; build richer schemas with the
/// <see cref="P"/> helpers or the <c>With*</c> methods. Mirrors kriya-core's <c>ParameterSchema</c>.
/// </summary>
public sealed class ParameterSchema
{
    public ParamType Type { get; }
    public string? Description { get; init; }
    public bool Required { get; init; }

    /// <summary>For <see cref="ParamType.Array"/>: the element schema.</summary>
    public ParameterSchema? Items { get; init; }

    /// <summary>For <see cref="ParamType.Object"/>: the nested property schemas.</summary>
    public IReadOnlyDictionary<string, ParameterSchema>? Properties { get; init; }

    /// <summary>Restrict to an enumerated set of values (strings or numbers).</summary>
    public IReadOnlyList<object>? Enum { get; init; }

    public ParameterSchema(ParamType type) => Type = type;

    /// <summary>Return a copy marked required.</summary>
    public ParameterSchema AsRequired() => new(Type)
    {
        Description = Description,
        Required = true,
        Items = Items,
        Properties = Properties,
        Enum = Enum,
    };

    public ParameterSchema WithDescription(string description) => new(Type)
    {
        Description = description,
        Required = Required,
        Items = Items,
        Properties = Properties,
        Enum = Enum,
    };
}

/// <summary>Ready-made parameter-schema helpers — the C# equivalent of kriya-core's <c>str</c>/<c>num</c>.</summary>
public static class P
{
    public static ParameterSchema Str => new(ParamType.String);
    public static ParameterSchema Num => new(ParamType.Number);
    public static ParameterSchema Bool => new(ParamType.Boolean);

    public static ParameterSchema Array(ParameterSchema items, string? description = null, bool required = false) =>
        new(ParamType.Array) { Items = items, Description = description, Required = required };

    public static ParameterSchema Obj(IDictionary<string, ParameterSchema> properties, string? description = null, bool required = false) =>
        new(ParamType.Object) { Properties = new Dictionary<string, ParameterSchema>(properties), Description = description, Required = required };

    /// <summary>Mark a schema required, e.g. <c>P.Required(P.Str)</c>.</summary>
    public static ParameterSchema Required(ParameterSchema schema) => schema.AsRequired();
}

/// <summary>A permission scope, e.g. <c>"write:notes"</c>. Checked by the host's policy.</summary>
public readonly record struct Permission(string Scope)
{
    public static implicit operator Permission(string scope) => new(scope);
    public override string ToString() => Scope;
}

/// <summary>What an action handler returns. <see cref="Error"/> is set only when not successful.</summary>
public sealed class ActionResult
{
    public bool Success { get; init; }
    public object? Data { get; init; }
    public string? Error { get; init; }

    public static ActionResult Ok(object? data = null) => new() { Success = true, Data = data };
    public static ActionResult Err(string message) => new() { Success = false, Error = message };
}

/// <summary>Context handed to a handler at execution time.</summary>
public sealed class ActionContext
{
    /// <summary>"human" or "agent".</summary>
    public string Caller { get; init; } = "human";

    /// <summary>The agent step id this call belongs to, when <see cref="Caller"/> is "agent".</summary>
    public string? StepId { get; init; }

    /// <summary>Composition: invoke a child action from inside a handler (same validation + audit path).</summary>
    public Func<string, JsonObject, ActionResult>? Call { get; init; }

    /// <summary>Action ids the current call descends from, parent-first (cycle/depth guard, read-only).</summary>
    public IReadOnlyList<string> Chain { get; init; } = System.Array.Empty<string>();
}

/// <summary>An action handler: receives the agent's params + context, returns an <see cref="ActionResult"/>.</summary>
public delegate ActionResult Handler(JsonObject parameters, ActionContext ctx);

/// <summary>The definition passed to <c>RegisterAction</c>.</summary>
public sealed class ActionDefinition
{
    public required string Id { get; init; }
    public required string Description { get; init; }
    public required Handler Handler { get; init; }
    public IReadOnlyDictionary<string, ParameterSchema> Parameters { get; init; } = new Dictionary<string, ParameterSchema>();
    public IReadOnlyList<string> Permissions { get; init; } = System.Array.Empty<string>();
    public int Version { get; init; } = 1;
}
