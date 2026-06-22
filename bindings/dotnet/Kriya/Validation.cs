using System.Text.Json;
using System.Text.Json.Nodes;

namespace Kriya;

/// <summary>One problem found validating params against their schemas.</summary>
public sealed record ValidationIssue(string Path, string Message);

/// <summary>
/// Runtime validation of action parameters against their declared schemas — what stops an agent (or
/// a buggy caller) invoking a handler with the wrong shape. Types, enums, required-ness, array
/// element and object property types. Unknown params are ignored (forward-compatible). Faithful port
/// of kriya-core's <c>validate.ts</c>.
/// </summary>
public static class Validation
{
    private static string TypeOf(JsonNode? node)
    {
        if (node is null) return "null";
        return node.GetValueKind() switch
        {
            JsonValueKind.String => "string",
            JsonValueKind.Number => "number",
            JsonValueKind.True or JsonValueKind.False => "boolean",
            JsonValueKind.Array => "array",
            JsonValueKind.Object => "object",
            JsonValueKind.Null => "null",
            _ => "object",
        };
    }

    private static void Check(JsonNode? value, ParameterSchema schema, string path, List<ValidationIssue> issues)
    {
        var actual = TypeOf(value);
        var expected = JsonSchema.TypeName(schema.Type);
        if (actual != expected)
        {
            issues.Add(new ValidationIssue(path, $"expected {expected}, got {actual}"));
            return; // type wrong — deeper checks would be noise
        }

        if (schema.Enum is not null)
        {
            var asJson = value?.ToJsonString();
            var ok = schema.Enum.Any(e => JsonValue.Create(e)?.ToJsonString() == asJson);
            if (!ok)
                issues.Add(new ValidationIssue(path, $"value {asJson} is not one of the allowed values"));
        }

        if (schema.Type == ParamType.Array && schema.Items is not null && value is JsonArray arr)
        {
            for (var i = 0; i < arr.Count; i++)
                Check(arr[i], schema.Items, $"{path}[{i}]", issues);
        }

        if (schema.Type == ParamType.Object && schema.Properties is not null && value is JsonObject obj)
        {
            foreach (var (key, prop) in schema.Properties)
            {
                if (!obj.ContainsKey(key))
                {
                    if (prop.Required) issues.Add(new ValidationIssue($"{path}.{key}", "required"));
                    continue;
                }
                Check(obj[key], prop, $"{path}.{key}", issues);
            }
        }
    }

    /// <summary>Validate a params object against a map of named parameter schemas.</summary>
    public static List<ValidationIssue> ValidateParams(JsonObject parameters, IReadOnlyDictionary<string, ParameterSchema> schemas)
    {
        var issues = new List<ValidationIssue>();
        foreach (var (name, schema) in schemas)
        {
            if (!parameters.ContainsKey(name))
            {
                if (schema.Required) issues.Add(new ValidationIssue(name, "required"));
                continue;
            }
            Check(parameters[name], schema, name, issues);
        }
        return issues;
    }

    /// <summary>Format issues into a single human/agent-readable string.</summary>
    public static string FormatIssues(IEnumerable<ValidationIssue> issues) =>
        string.Join("; ", issues.Select(i => $"{i.Path}: {i.Message}"));
}
