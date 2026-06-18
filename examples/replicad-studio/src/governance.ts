/**
 * The in-app governance layer — the same posture as the bolt-on's `agent-policy.yaml`. Every action
 * an agent (or you) takes is decided here BEFORE it touches the model: reads + routine edits flow;
 * destructive ops require approval; anything unlisted is denied. (In the Tauri/host build this is
 * where Ed25519-signed receipts are written; in this browser studio we surface the decision + an
 * audit trail.)
 */

import { defaultModel, PARAM_KEYS, type CadModel, type ParamKey } from "./cadModel";

export type Tier = "allow" | "approval" | "deny";

export const TIER_LABEL: Record<Tier, string> = {
  allow: "Allow",
  approval: "Requires approval",
  deny: "Deny",
};

// Mirrors examples/replicad-bolt-on/agent-policy.yaml (first match wins, no match = deny).
const POLICY: { pattern: string; tier: Tier }[] = [
  { pattern: "measure", tier: "allow" },
  { pattern: "set_parameter", tier: "allow" },
  { pattern: "add_hole", tier: "allow" },
  { pattern: "delete_feature", tier: "approval" },
  { pattern: "delete_body", tier: "approval" },
  { pattern: "reset_model", tier: "approval" },
  { pattern: "*", tier: "deny" },
];

export function decide(actionId: string): Tier {
  for (const rule of POLICY) {
    if (rule.pattern === "*" || rule.pattern === actionId) return rule.tier;
  }
  return "deny";
}

let holeSeq = 5;

/**
 * Apply an already-cleared action to the model (mutates `m`), returning a human summary.
 * Throws on an invalid request — the caller turns that into a failed audit entry.
 */
export function applyAction(m: CadModel, id: string, p: Record<string, unknown>): string {
  switch (id) {
    case "set_parameter": {
      const name = String(p.name);
      const value = Number(p.value);
      if (!(PARAM_KEYS as readonly string[]).includes(name)) throw new Error(`unknown parameter "${name}"`);
      if (!(value > 0)) throw new Error("value must be > 0");
      m.params[name as ParamKey] = value;
      return `${name} → ${value} mm`;
    }
    case "add_hole": {
      if (!m.hasBody) throw new Error("no body to drill");
      const diameter = Number(p.diameter);
      if (!(diameter > 0)) throw new Error("diameter must be > 0");
      const id2 = `h${holeSeq++}`;
      m.holes.push({ id: id2, x: Number(p.x) || 0, y: Number(p.y) || 0, diameter });
      return `drilled ${id2} (⌀${diameter}) — ${m.holes.length} holes`;
    }
    case "delete_feature": {
      const i = m.holes.findIndex((h) => h.id === p.id);
      if (i < 0) throw new Error(`no feature "${p.id}"`);
      m.holes.splice(i, 1);
      return `deleted ${p.id} — ${m.holes.length} holes`;
    }
    case "delete_body": {
      if (!m.hasBody) throw new Error("body already deleted");
      m.hasBody = false;
      m.holes = [];
      return "solid body deleted";
    }
    case "reset_model": {
      const d = defaultModel();
      m.params = d.params;
      m.holes = d.holes;
      m.hasBody = true;
      holeSeq = 5;
      return "model reset to default plate";
    }
    default:
      throw new Error(`unknown action "${id}"`);
  }
}
