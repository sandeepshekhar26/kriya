using System.Text.Json.Nodes;
using Kriya;

namespace Kriya.Tests;

public class JsonSchemaTests
{
    [Fact]
    public void RequiredIsLiftedToTheParentArrayNotAPerPropertyBool()
    {
        var schemas = new Dictionary<string, ParameterSchema>
        {
            ["title"] = P.Required(P.Str),
            ["body"] = P.Str,
        };
        var s = JsonSchema.ParamsToJsonSchema(schemas);
        Assert.Equal("object", s["type"]!.GetValue<string>());

        var required = s["required"]!.AsArray().Select(n => n!.GetValue<string>()).ToList();
        Assert.Contains("title", required);
        Assert.DoesNotContain("body", required);

        // The internal per-property `required` boolean must NOT leak into the emitted schema.
        Assert.False(s["properties"]!["title"]!.AsObject().ContainsKey("required"));
    }

    [Fact]
    public void ArrayItemsAreExpandedToASchemaObject()
    {
        var s = JsonSchema.ParamsToJsonSchema(new Dictionary<string, ParameterSchema> { ["tags"] = P.Array(P.Str) });
        var items = s["properties"]!["tags"]!["items"]!.AsObject();
        Assert.Equal("string", items["type"]!.GetValue<string>());
    }

    [Fact]
    public void NoRequiredKeyWhenNothingIsRequired()
    {
        var s = JsonSchema.ParamsToJsonSchema(new Dictionary<string, ParameterSchema> { ["x"] = P.Str });
        Assert.False(s.ContainsKey("required"));
    }
}
