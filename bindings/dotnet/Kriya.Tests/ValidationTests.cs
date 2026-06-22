using System.Text.Json.Nodes;
using Kriya;

namespace Kriya.Tests;

public class ValidationTests
{
    [Fact]
    public void TypeMismatchIsAnIssue()
    {
        var issues = Validation.ValidateParams(
            new JsonObject { ["n"] = "notnum" },
            new Dictionary<string, ParameterSchema> { ["n"] = P.Num });
        var issue = Assert.Single(issues);
        Assert.Contains("expected number", issue.Message);
    }

    [Fact]
    public void MissingRequiredIsAnIssue()
    {
        var issues = Validation.ValidateParams(
            new JsonObject(),
            new Dictionary<string, ParameterSchema> { ["title"] = P.Required(P.Str) });
        Assert.Equal("required", Assert.Single(issues).Message);
    }

    [Fact]
    public void ValidParamsPass()
    {
        var issues = Validation.ValidateParams(
            new JsonObject { ["title"] = "hi", ["n"] = 3 },
            new Dictionary<string, ParameterSchema> { ["title"] = P.Required(P.Str), ["n"] = P.Num });
        Assert.Empty(issues);
    }

    [Fact]
    public void BooleanIsNotANumber()
    {
        var issues = Validation.ValidateParams(
            new JsonObject { ["n"] = true },
            new Dictionary<string, ParameterSchema> { ["n"] = P.Num });
        Assert.Single(issues);
    }

    [Fact]
    public void ArrayElementTypesAreChecked()
    {
        var bad = new JsonObject { ["tags"] = new JsonArray("a", 2, "c") };
        var issues = Validation.ValidateParams(bad, new Dictionary<string, ParameterSchema> { ["tags"] = P.Array(P.Str) });
        Assert.Contains("tags[1]", Assert.Single(issues).Path);
    }

    [Fact]
    public void UnknownParamsAreIgnored()
    {
        var issues = Validation.ValidateParams(
            new JsonObject { ["title"] = "hi", ["extra"] = 1 },
            new Dictionary<string, ParameterSchema> { ["title"] = P.Str });
        Assert.Empty(issues);
    }
}
