/**
 * The `wrapAction` codemod: scan a source file's exported functions and scaffold
 * `wrapAction(...)` registrations for them — so bolting kriya onto an existing app is a
 * generate-then-tweak step, not hand-writing a schema per handler. **Augment, not migrate.**
 *
 * It reads the function signatures with the TypeScript compiler API to infer each parameter's
 * schema (string/number/boolean/array/object) and whether it's required, then prints a module
 * the developer reviews (fill in descriptions, adjust types) and imports at boot.
 *
 * The TypeScript compiler is loaded lazily so the runtime SDK never depends on it — only the
 * `kriya wrap` CLI path pays for it.
 */

import type * as TS from "typescript";

import type { ParameterType } from "./types.js";

async function loadTypeScript(): Promise<typeof TS> {
  try {
    const mod = (await import("typescript")) as unknown as {
      default?: typeof TS;
    } & typeof TS;
    return mod.default ?? mod;
  } catch {
    throw new Error(
      "`kriya wrap` needs the TypeScript compiler. Install it: npm i -D typescript",
    );
  }
}

/** One parameter the agent will see, as inferred from the function signature. */
interface ParamInfo {
  name: string;
  type: ParameterType;
  items?: ParameterType;
  required: boolean;
}

/** An exported function discovered in the source, ready to scaffold. */
interface FnInfo {
  name: string;
  jsdoc?: string;
  params: ParamInfo[];
  /** True when the function takes a single object argument — the params object maps directly,
   * so no `mapParams` is needed. */
  singleObjectParam: boolean;
}

export interface ScaffoldOptions {
  /** Module specifier the generated file imports the wrapped functions from, e.g. "./actions.js". */
  importPath: string;
}

/**
 * Produce the text of a registration module for every exported function in `sourceText`.
 * Returns an empty string when no exported functions are found.
 */
export async function scaffoldWrappers(
  sourceText: string,
  options: ScaffoldOptions,
): Promise<string> {
  const ts = await loadTypeScript();
  const sf = ts.createSourceFile("input.ts", sourceText, ts.ScriptTarget.Latest, true);
  const fns = collectExportedFunctions(ts, sf);
  if (fns.length === 0) return "";
  return renderModule(fns, options.importPath);
}

function hasExportModifier(ts: typeof TS, node: TS.Node): boolean {
  if (!ts.canHaveModifiers(node)) return false;
  return (ts.getModifiers(node) ?? []).some((m) => m.kind === ts.SyntaxKind.ExportKeyword);
}

function collectExportedFunctions(ts: typeof TS, sf: TS.SourceFile): FnInfo[] {
  const out: FnInfo[] = [];
  for (const stmt of sf.statements) {
    if (ts.isFunctionDeclaration(stmt) && stmt.name && hasExportModifier(ts, stmt)) {
      out.push(fromSignature(ts, stmt.name.text, stmt.parameters, jsdocOf(ts, stmt)));
    } else if (ts.isVariableStatement(stmt) && hasExportModifier(ts, stmt)) {
      for (const decl of stmt.declarationList.declarations) {
        const init = decl.initializer;
        if (
          ts.isIdentifier(decl.name) &&
          init &&
          (ts.isArrowFunction(init) || ts.isFunctionExpression(init))
        ) {
          out.push(fromSignature(ts, decl.name.text, init.parameters, jsdocOf(ts, stmt)));
        }
      }
    }
  }
  return out;
}

function jsdocOf(ts: typeof TS, node: TS.Node): string | undefined {
  const tags = ts.getJSDocCommentsAndTags(node);
  for (const tag of tags) {
    const comment = (tag as TS.JSDoc).comment;
    if (typeof comment === "string" && comment.trim()) {
      // First line only — descriptions are one-liners.
      return comment.trim().split("\n")[0]?.trim();
    }
  }
  return undefined;
}

function fromSignature(
  ts: typeof TS,
  name: string,
  parameters: readonly TS.ParameterDeclaration[],
  jsdoc: string | undefined,
): FnInfo {
  // Single object-typed parameter → the agent's params object maps straight through.
  const only = parameters.length === 1 ? parameters[0] : undefined;
  if (only && only.type && ts.isTypeLiteralNode(only.type)) {
    const params = only.type.members
      .filter(ts.isPropertySignature)
      .map((m): ParamInfo => {
        const pname = m.name && ts.isIdentifier(m.name) ? m.name.text : "param";
        const { type, items } = inferType(ts, m.type);
        return { name: pname, type, items, required: !m.questionToken };
      });
    return { name, jsdoc, params, singleObjectParam: true };
  }

  // Positional parameters: each becomes a top-level param, threaded via mapParams.
  const params = parameters.map((p): ParamInfo => {
    const pname = ts.isIdentifier(p.name) ? p.name.text : "param";
    const { type, items } = inferType(ts, p.type);
    return { name: pname, type, items, required: !p.questionToken && !p.initializer };
  });
  return { name, jsdoc, params, singleObjectParam: false };
}

function inferType(
  ts: typeof TS,
  typeNode: TS.TypeNode | undefined,
): { type: ParameterType; items?: ParameterType } {
  if (!typeNode) return { type: "string" };
  switch (typeNode.kind) {
    case ts.SyntaxKind.StringKeyword:
      return { type: "string" };
    case ts.SyntaxKind.NumberKeyword:
      return { type: "number" };
    case ts.SyntaxKind.BooleanKeyword:
      return { type: "boolean" };
    default:
      break;
  }
  if (ts.isArrayTypeNode(typeNode)) {
    return { type: "array", items: inferType(ts, typeNode.elementType).type };
  }
  if (ts.isTypeReferenceNode(typeNode) && ts.isIdentifier(typeNode.typeName)) {
    if (typeNode.typeName.text === "Array" && typeNode.typeArguments?.[0]) {
      return { type: "array", items: inferType(ts, typeNode.typeArguments[0]).type };
    }
  }
  // Type literals, interfaces, custom refs → object.
  if (ts.isTypeLiteralNode(typeNode) || ts.isTypeReferenceNode(typeNode)) {
    return { type: "object" };
  }
  return { type: "string" };
}

// camelCase / PascalCase → snake_case, matching the registry's action-id convention.
function toSnakeCase(name: string): string {
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[\s-]+/g, "_")
    .toLowerCase();
}

function escapeString(s: string): string {
  return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function renderParam(p: ParamInfo): string {
  const parts = [`type: "${p.type}"`];
  if (p.items) parts.push(`items: "${p.items}"`);
  if (p.required) parts.push("required: true");
  return `    ${p.name}: { ${parts.join(", ")} },`;
}

function renderOne(fn: FnInfo): string {
  const id = toSnakeCase(fn.name);
  const description = fn.jsdoc
    ? escapeString(fn.jsdoc)
    : `TODO: describe ${id} for the agent.`;

  const lines: string[] = [];
  lines.push(`wrapAction(${fn.name}, {`);
  lines.push(`  id: "${id}",`);
  lines.push(`  description: "${description}",`);
  if (fn.params.length > 0) {
    lines.push("  parameters: {");
    for (const p of fn.params) lines.push(renderParam(p));
    lines.push("  },");
  } else {
    lines.push("  parameters: {},");
  }
  if (!fn.singleObjectParam && fn.params.length > 0) {
    const args = fn.params.map((p) => `p.${p.name}`).join(", ");
    lines.push(`  mapParams: (p) => [${args}],`);
  }
  lines.push("});");
  return lines.join("\n");
}

function renderModule(fns: FnInfo[], importPath: string): string {
  const names = fns.map((f) => f.name).join(", ");
  const header = [
    "// AUTO-GENERATED by `kriya wrap`. Review each description + TODO and the inferred",
    "// parameter types, then import this module where your app boots so the wrappers register.",
    'import { wrapAction } from "kriya-core";',
    `import { ${names} } from "${importPath}";`,
    "",
    "",
  ].join("\n");
  return header + fns.map(renderOne).join("\n\n") + "\n";
}
