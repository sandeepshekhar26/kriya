using System.Text.Json.Nodes;
using Kriya;

namespace Kriya.Tests;

public class RegistryTests
{
    [Fact]
    public void RegistersAndDispatches()
    {
        var reg = new Registry();
        reg.RegisterAction("create_note", "Create a note.",
            (p, ctx) => ActionResult.Ok(new JsonObject { ["id"] = 1 }),
            parameters: new Dictionary<string, ParameterSchema> { ["title"] = P.Required(P.Str) });
        var r = reg.DispatchAction("create_note", new JsonObject { ["title"] = "hi" }, new ActionContext());
        Assert.True(r.Success);
    }

    [Fact]
    public void RejectsInvalidId() =>
        Assert.Throws<ActionValidationException>(() =>
            new Registry().RegisterAction("Bad-Id", "x", (p, c) => ActionResult.Ok()));

    [Fact]
    public void RejectsDuplicate()
    {
        var reg = new Registry();
        reg.RegisterAction("a", "x", (p, c) => ActionResult.Ok());
        Assert.Throws<ActionValidationException>(() => reg.RegisterAction("a", "y", (p, c) => ActionResult.Ok()));
    }

    [Fact]
    public void ValidatesParamsOnDispatch()
    {
        var reg = new Registry();
        reg.RegisterAction("create_note", "Create a note.", (p, c) => ActionResult.Ok(),
            parameters: new Dictionary<string, ParameterSchema> { ["title"] = P.Required(P.Str) });
        var r = reg.DispatchAction("create_note", new JsonObject(), new ActionContext()); // missing title
        Assert.False(r.Success);
        Assert.Contains("Invalid parameters", r.Error);
    }

    [Fact]
    public void UnknownActionFails()
    {
        var r = new Registry().DispatchAction("nope", new JsonObject(), new ActionContext());
        Assert.False(r.Success);
        Assert.Contains("Unknown action", r.Error);
    }

    [Fact]
    public void CompositionCycleIsDetected()
    {
        var reg = new Registry();
        reg.RegisterAction("a", "x", (p, ctx) => ctx.Call!("b", new JsonObject()));
        reg.RegisterAction("b", "x", (p, ctx) => ctx.Call!("a", new JsonObject()));
        var r = reg.DispatchAction("a", new JsonObject(), new ActionContext());
        Assert.False(r.Success);
        Assert.Contains("cycle", r.Error);
    }

    [Fact]
    public void CompositionRunsAChildAction()
    {
        var reg = new Registry();
        reg.RegisterAction("child", "x", (p, ctx) => ActionResult.Ok("child-ran"));
        reg.RegisterAction("parent", "x", (p, ctx) => ctx.Call!("child", new JsonObject()));
        var r = reg.DispatchAction("parent", new JsonObject(), new ActionContext());
        Assert.True(r.Success);
        Assert.Equal("child-ran", r.Data);
    }

    [Fact]
    public void WrapActionAdaptsAFunction()
    {
        var reg = new Registry();
        reg.WrapAction(args => $"deleted {args[0]}", id: "delete_note", description: "Delete a note.",
            parameters: new Dictionary<string, ParameterSchema> { ["id"] = P.Required(P.Num) },
            mapParams: p => new object?[] { p["id"]!.GetValue<int>() });
        var r = reg.DispatchAction("delete_note", new JsonObject { ["id"] = 7 }, new ActionContext());
        Assert.True(r.Success);
        Assert.Equal("deleted 7", r.Data);
    }

    [Fact]
    public void WrapActionTurnsThrowIntoError()
    {
        var reg = new Registry();
        reg.WrapAction(args => throw new InvalidOperationException("boom"), id: "boom", description: "x");
        var r = reg.DispatchAction("boom", new JsonObject(), new ActionContext());
        Assert.False(r.Success);
        Assert.Equal("boom", r.Error);
    }

    [Fact]
    public void ToolSchemasEmitCamelCaseInputSchema()
    {
        var reg = new Registry();
        reg.RegisterAction("create_note", "Create a note.", (p, c) => ActionResult.Ok(),
            parameters: new Dictionary<string, ParameterSchema> { ["title"] = P.Required(P.Str) },
            permissions: new[] { "write:notes" });
        var tools = reg.ToolSchemas();
        var t = Assert.Single(tools);
        Assert.Equal("create_note", t["name"]!.GetValue<string>());
        var input = t["inputSchema"]!.AsObject();
        Assert.Equal("object", input["type"]!.GetValue<string>());
        Assert.Equal("title", input["required"]!.AsArray()[0]!.GetValue<string>());
    }
}
