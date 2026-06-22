using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace Kriya;

/// <summary>Thrown when an action <em>definition</em> is invalid (bad id, duplicate, missing handler…).</summary>
public sealed class ActionValidationException : Exception
{
    public ActionValidationException(string message) : base(message) { }
}

/// <summary>A callable handle returned by <see cref="Registry.RegisterAction"/> (handy for tests).</summary>
public sealed class RegisteredAction
{
    private readonly Registry _registry;
    public string Id { get; }

    internal RegisteredAction(Registry registry, string id)
    {
        _registry = registry;
        Id = id;
    }

    public ActionResult Call(JsonObject parameters, ActionContext? ctx = null) =>
        _registry.DispatchAction(Id, parameters, ctx ?? new ActionContext { Caller = "human" });
}

/// <summary>
/// The action registry: a map of registered actions. Hands the host an MCP-compatible tool schema of
/// every action (no handlers) and dispatches an incoming agent tool-call to the right handler —
/// through the same validation + composition path as kriya-core.
/// </summary>
public sealed class Registry
{
    /// <summary>Max nested-composition depth (a handler calling a child calling another…).</summary>
    public const int MaxComposeDepth = 8;

    private static readonly Regex IdPattern = new("^[a-z][a-z0-9_]*$", RegexOptions.Compiled);
    private readonly Dictionary<string, ActionDefinition> _actions = new();

    /// <summary>Register an action so agents can discover and call it.</summary>
    public RegisteredAction RegisterAction(
        string id,
        string description,
        Handler handler,
        Dictionary<string, ParameterSchema>? parameters = null,
        IEnumerable<string>? permissions = null,
        int version = 1)
    {
        var def = new ActionDefinition
        {
            Id = id,
            Description = description,
            Handler = handler,
            Parameters = parameters is null
                ? new Dictionary<string, ParameterSchema>()
                : new Dictionary<string, ParameterSchema>(parameters),
            Permissions = permissions?.ToArray() ?? System.Array.Empty<string>(),
            Version = version,
        };
        ValidateDefinition(def);
        _actions[def.Id] = def;
        return new RegisteredAction(this, def.Id);
    }

    /// <summary>
    /// Bolt the kriya layer onto a function the app <em>already has</em> — augment, not migrate.
    /// <paramref name="fn"/> takes the mapped argument array and returns a plain value (or throws);
    /// a returned value becomes <c>Ok(data)</c>, a throw becomes <c>Err(message)</c>.
    /// </summary>
    public RegisteredAction WrapAction(
        Func<object?[], object?> fn,
        string id,
        string description,
        Dictionary<string, ParameterSchema>? parameters = null,
        IEnumerable<string>? permissions = null,
        int version = 1,
        Func<JsonObject, object?[]>? mapParams = null,
        Func<object?, object?>? mapResult = null)
    {
        Handler handler = (parameters_, _ctx) =>
        {
            object? returned;
            try
            {
                var args = mapParams is not null ? mapParams(parameters_) : new object?[] { parameters_ };
                returned = fn(args);
            }
            catch (Exception ex)
            {
                // The app's own throw is an expected failure mode, not a crash.
                return ActionResult.Err(ex.Message);
            }
            var data = mapResult is not null ? mapResult(returned) : returned;
            return ActionResult.Ok(data);
        };
        return RegisterAction(id, description, handler, parameters, permissions, version);
    }

    private void ValidateDefinition(ActionDefinition def)
    {
        if (string.IsNullOrEmpty(def.Id) || !IdPattern.IsMatch(def.Id))
            throw new ActionValidationException($"Action id \"{def.Id}\" is invalid. Use snake_case starting with a letter.");
        if (_actions.ContainsKey(def.Id))
            throw new ActionValidationException($"Action \"{def.Id}\" is already registered.");
        if (string.IsNullOrWhiteSpace(def.Description))
            throw new ActionValidationException($"Action \"{def.Id}\" needs a non-empty description.");
        foreach (var (name, schema) in def.Parameters)
        {
            if (schema.Type == ParamType.Array && schema.Items is null)
                throw new ActionValidationException($"Action \"{def.Id}\" parameter \"{name}\" is an array but has no items.");
        }
    }

    /// <summary>Look up and run a registered action by id (used when an agent calls a tool).</summary>
    public ActionResult DispatchAction(string id, JsonObject parameters, ActionContext ctx) =>
        DispatchWithChain(id, parameters, ctx, ctx.Chain ?? System.Array.Empty<string>());

    private ActionResult DispatchWithChain(string id, JsonObject parameters, ActionContext ctx, IReadOnlyList<string> parentChain)
    {
        if (parentChain.Contains(id))
            return ActionResult.Err($"Composition cycle detected: {string.Join(" -> ", parentChain.Append(id))}.");
        if (parentChain.Count >= MaxComposeDepth)
            return ActionResult.Err($"Composition depth {MaxComposeDepth} exceeded at \"{id}\" (chain: {string.Join(" -> ", parentChain)}).");

        if (!_actions.TryGetValue(id, out var def))
            return ActionResult.Err($"Unknown action \"{id}\".");

        var issues = Validation.ValidateParams(parameters, def.Parameters);
        if (issues.Count > 0)
            return ActionResult.Err($"Invalid parameters: {Validation.FormatIssues(issues)}.");

        var newChain = parentChain.Append(id).ToList();
        var childCtx = new ActionContext
        {
            Caller = ctx.Caller,
            StepId = ctx.StepId,
            Chain = newChain,
            Call = (childId, childParams) => DispatchWithChain(childId, childParams,
                new ActionContext { Caller = ctx.Caller, StepId = ctx.StepId, Chain = newChain }, newChain),
        };

        try
        {
            return def.Handler(parameters, childCtx);
        }
        catch (Exception ex)
        {
            return ActionResult.Err(ex.Message);
        }
    }

    /// <summary>MCP-compatible tool schemas for every registered action (no handlers). camelCase wire keys.</summary>
    public List<JsonObject> ToolSchemas()
    {
        var list = new List<JsonObject>();
        foreach (var def in _actions.Values)
        {
            var permissions = new JsonArray();
            foreach (var p in def.Permissions) permissions.Add((JsonNode)p);
            list.Add(new JsonObject
            {
                ["name"] = def.Id,
                ["version"] = def.Version,
                ["description"] = def.Description,
                ["permissions"] = permissions,
                ["inputSchema"] = JsonSchema.ParamsToJsonSchema(def.Parameters),
            });
        }
        return list;
    }

    /// <summary>Like <see cref="ToolSchemas"/> but the bare MCP shape (no kriya <c>permissions</c>).</summary>
    public List<JsonObject> McpToolSchemas()
    {
        var list = new List<JsonObject>();
        foreach (var t in ToolSchemas())
        {
            list.Add(new JsonObject
            {
                ["name"] = t["name"]?.DeepClone(),
                ["version"] = t["version"]?.DeepClone(),
                ["description"] = t["description"]?.DeepClone(),
                ["inputSchema"] = t["inputSchema"]?.DeepClone(),
            });
        }
        return list;
    }

    public IReadOnlyList<string> ListActionIds() => _actions.Keys.ToList();
    public ActionDefinition? Get(string id) => _actions.TryGetValue(id, out var d) ? d : null;
    public void Clear() => _actions.Clear();
}
