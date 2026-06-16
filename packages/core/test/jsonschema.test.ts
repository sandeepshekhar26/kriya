import { afterEach, describe, expect, it } from "vitest";
import { clearRegistry, getMcpToolSchemas, registerAction } from "../src/registry.js";
import { toJSONSchema, paramsToJSONSchema } from "../src/jsonschema.js";
import type { ParameterSchema } from "../src/types.js";

afterEach(() => clearRegistry());

describe("toJSONSchema", () => {
  it("converts a plain string to { type: 'string' }", () => {
    const schema: ParameterSchema = { type: "string" };
    expect(toJSONSchema(schema)).toEqual({ type: "string" });
  });

  it("includes description when present", () => {
    const schema: ParameterSchema = { type: "number", description: "A count." };
    expect(toJSONSchema(schema)).toEqual({ type: "number", description: "A count." });
  });

  it("string with enum -> { type: 'string', enum: [...] }", () => {
    const schema: ParameterSchema = {
      type: "string",
      enum: ["open", "done", "archived"],
    };
    expect(toJSONSchema(schema)).toEqual({
      type: "string",
      enum: ["open", "done", "archived"],
    });
  });

  it("array of strings (bare string items) -> { type:'array', items:{ type:'string' } }", () => {
    const schema: ParameterSchema = { type: "array", items: "string" };
    expect(toJSONSchema(schema)).toEqual({
      type: "array",
      items: { type: "string" },
    });
  });

  it("array with a ParameterSchema items -> items is a schema object", () => {
    const schema: ParameterSchema = {
      type: "array",
      items: { type: "number", description: "A score." },
    };
    expect(toJSONSchema(schema)).toEqual({
      type: "array",
      items: { type: "number", description: "A score." },
    });
  });

  it("nested object with a required child -> required array at the right level", () => {
    const schema: ParameterSchema = {
      type: "object",
      properties: {
        id: { type: "string", required: true },
        note: { type: "string" },
      },
    };
    const result = toJSONSchema(schema) as Record<string, unknown>;

    // required array is present and contains only "id"
    expect(result["required"]).toEqual(["id"]);

    // per-property boolean must not appear anywhere in properties
    const props = result["properties"] as Record<string, Record<string, unknown>>;
    expect(props["id"]).not.toHaveProperty("required");
    expect(props["note"]).not.toHaveProperty("required");
  });

  it("omits 'required' array when no child properties are required", () => {
    const schema: ParameterSchema = {
      type: "object",
      properties: {
        title: { type: "string" },
        body: { type: "string" },
      },
    };
    const result = toJSONSchema(schema) as Record<string, unknown>;
    expect(result).not.toHaveProperty("required");
  });

  it("does not emit the per-property required boolean for primitive types", () => {
    // required is an internal hint; it must never appear in JSON Schema output
    const schema: ParameterSchema = { type: "string", required: true };
    const result = toJSONSchema(schema) as Record<string, unknown>;
    expect(result).not.toHaveProperty("required");
  });
});

describe("paramsToJSONSchema", () => {
  it("wraps a flat params map in an object schema", () => {
    const params: Record<string, ParameterSchema> = {
      title: { type: "string", required: true },
      count: { type: "number" },
    };
    expect(paramsToJSONSchema(params)).toEqual({
      type: "object",
      properties: {
        title: { type: "string" },
        count: { type: "number" },
      },
      required: ["title"],
    });
  });

  it("omits required array when no params are required", () => {
    const params: Record<string, ParameterSchema> = {
      note: { type: "string" },
    };
    const result = paramsToJSONSchema(params) as Record<string, unknown>;
    expect(result).not.toHaveProperty("required");
  });

  it("handles an empty params map", () => {
    expect(paramsToJSONSchema({})).toEqual({ type: "object", properties: {} });
  });
});

describe("getMcpToolSchemas", () => {
  it("returns [] when no actions are registered", () => {
    expect(getMcpToolSchemas()).toEqual([]);
  });

  it("produces standard JSON Schema for a full action", () => {
    registerAction({
      id: "create_note",
      version: 1,
      description: "Create a note.",
      parameters: {
        title: { type: "string", required: true, description: "Note title." },
        tags: { type: "array", items: "string" },
        status: { type: "string", enum: ["draft", "published"] },
      },
      permissions: ["write:notes"],
      handler: () => ({ success: true }),
    });

    const schemas = getMcpToolSchemas();
    expect(schemas).toHaveLength(1);

    const tool = schemas[0]!;
    expect(tool.name).toBe("create_note");
    expect(tool.version).toBe(1);
    expect(tool.description).toBe("Create a note.");

    const input = tool.inputSchema as Record<string, unknown>;
    expect(input["type"]).toBe("object");

    // required is an array of names, not per-property booleans
    expect(Array.isArray(input["required"])).toBe(true);
    expect(input["required"]).toEqual(["title"]);

    const props = input["properties"] as Record<string, Record<string, unknown>>;

    // title: description forwarded, no required boolean
    expect(props["title"]).toEqual({ type: "string", description: "Note title." });
    expect(props["title"]).not.toHaveProperty("required");

    // tags: items is a schema object, not a bare string
    expect(props["tags"]).toEqual({ type: "array", items: { type: "string" } });

    // status: enum forwarded
    expect(props["status"]).toEqual({ type: "string", enum: ["draft", "published"] });
  });

  it("matches name, version, description from the registration", () => {
    registerAction({
      id: "delete_note",
      version: 3,
      description: "Delete a note permanently.",
      parameters: { id: { type: "string", required: true } },
      handler: () => ({ success: true }),
    });

    const [tool] = getMcpToolSchemas();
    expect(tool!.name).toBe("delete_note");
    expect(tool!.version).toBe(3);
    expect(tool!.description).toBe("Delete a note permanently.");
  });

  it("no per-property required booleans remain anywhere in inputSchema", () => {
    registerAction({
      id: "edit_note",
      description: "Edit a note.",
      parameters: {
        id: { type: "string", required: true },
        metadata: {
          type: "object",
          properties: {
            author: { type: "string", required: true },
            draft: { type: "boolean" },
          },
        },
      },
      handler: () => ({ success: true }),
    });

    const [tool] = getMcpToolSchemas();
    const json = JSON.stringify(tool!.inputSchema);

    // The word "required" must only appear as a JSON array value, never as a
    // boolean. We verify by checking that the raw string has no `"required":true`
    // or `"required":false` patterns anywhere.
    expect(json).not.toMatch(/"required"\s*:\s*true/);
    expect(json).not.toMatch(/"required"\s*:\s*false/);

    // And the top-level required IS an array
    const input = tool!.inputSchema as Record<string, unknown>;
    expect(Array.isArray(input["required"])).toBe(true);

    // Nested object also has required as array, not boolean
    const nestedProps = (input["properties"] as Record<string, Record<string, unknown>>)[
      "metadata"
    ] as Record<string, unknown>;
    expect(Array.isArray(nestedProps["required"])).toBe(true);
    expect(nestedProps["required"]).toEqual(["author"]);
  });
});
