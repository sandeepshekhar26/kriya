import { describe, expect, it } from "vitest";
import { validateParams } from "../src/validate.js";
import type { ParameterSchema } from "../src/types.js";

describe("validateParams", () => {
  it("passes a well-formed object", () => {
    const schemas: Record<string, ParameterSchema> = {
      title: { type: "string", required: true },
      count: { type: "number" },
      tags: { type: "array", items: "string" },
    };
    expect(validateParams({ title: "x", count: 3, tags: ["a", "b"] }, schemas)).toEqual([]);
  });

  it("flags missing required params", () => {
    const issues = validateParams({}, { title: { type: "string", required: true } });
    expect(issues).toEqual([{ path: "title", message: "required" }]);
  });

  it("flags type mismatches", () => {
    const issues = validateParams({ count: "nope" }, { count: { type: "number" } });
    expect(issues[0]?.path).toBe("count");
    expect(issues[0]?.message).toMatch(/expected number/);
  });

  it("validates array element types", () => {
    const issues = validateParams(
      { tags: ["ok", 2] },
      { tags: { type: "array", items: "string" } }
    );
    expect(issues).toEqual([{ path: "tags[1]", message: "expected string, got number" }]);
  });

  it("enforces enum membership", () => {
    const schemas: Record<string, ParameterSchema> = {
      status: { type: "string", enum: ["open", "done"] },
    };
    expect(validateParams({ status: "open" }, schemas)).toEqual([]);
    expect(validateParams({ status: "wat" }, schemas)[0]?.message).toMatch(/not one of/);
  });

  it("ignores unknown params (forward-compatible)", () => {
    expect(validateParams({ extra: true }, { title: { type: "string" } })).toEqual([]);
  });
});
