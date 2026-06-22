using System.Text.Json.Nodes;

namespace Kriya;

/// <summary>
/// Standards-compliant JSON Schema (draft 2020-12 clean) export. kriya's compact
/// <see cref="ParameterSchema"/> uses a per-property <c>Required</c> bool and may carry a bare item
/// type; MCP clients want <c>required</c> as an array on the parent object and <c>items</c> as a
/// schema object. This converts between them, byte-faithful to kriya-core's <c>jsonschema.ts</c>.
/// </summary>
public static class JsonSchema
{
    internal static string TypeName(ParamType t) => t switch
    {
        ParamType.String => "string",
        ParamType.Number => "number",
        ParamType.Boolean => "boolean",
        ParamType.Array => "array",
        ParamType.Object => "object",
        _ => "object",
    };

    /// <summary>Convert one <see cref="ParameterSchema"/> node to a JSON Schema object.</summary>
    public static JsonObject ToJsonSchema(ParameterSchema schema)
    {
        var node = new JsonObject { ["type"] = TypeName(schema.Type) };

        if (schema.Description is not null)
            node["description"] = schema.Description;

        if (schema.Enum is not null)
        {
            var arr = new JsonArray();
            foreach (var v in schema.Enum)
                arr.Add(JsonValue.Create(v));
            node["enum"] = arr;
        }

        switch (schema.Type)
        {
            case ParamType.Array:
                // Items is guaranteed non-null for arrays by the registry validator.
                node["items"] = schema.Items is not null ? ToJsonSchema(schema.Items) : new JsonObject();
                break;

            case ParamType.Object when schema.Properties is not null:
                var props = new JsonObject();
                var required = new JsonArray();
                foreach (var (key, prop) in schema.Properties)
                {
                    props[key] = ToJsonSchema(prop);
                    if (prop.Required) required.Add((JsonNode)key);
                }
                node["properties"] = props;
                if (required.Count > 0) node["required"] = required;
                break;
        }

        return node;
    }

    /// <summary>Build a top-level object schema from a flat map of named parameter schemas.</summary>
    public static JsonObject ParamsToJsonSchema(IReadOnlyDictionary<string, ParameterSchema> parameters)
    {
        var props = new JsonObject();
        var required = new JsonArray();
        foreach (var (name, schema) in parameters)
        {
            props[name] = ToJsonSchema(schema);
            if (schema.Required) required.Add((JsonNode)name);
        }

        var result = new JsonObject { ["type"] = "object", ["properties"] = props };
        if (required.Count > 0) result["required"] = required;
        return result;
    }
}
