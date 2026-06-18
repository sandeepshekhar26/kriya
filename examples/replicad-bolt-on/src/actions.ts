/**
 * THE BOLT-ON. This is the entire integration: wrap the CAD app's existing in-process functions as
 * governed, agent-callable actions — no rewrite, no new API. Each `wrapAction` maps the agent's
 * params onto a method and normalizes the result; the host enforces the policy (which of these need
 * human approval) and signs an audit receipt per call.
 *
 * Reads + routine edits (resize, drill) flow; destructive ops (delete a feature/body, reset) are
 * gated by the policy. That is the whole governance story for a CAD model, in under 50 lines.
 */

import { wrapAction } from "kriya-core";
import type { CadApp } from "./cad.js";

const num = { type: "number", required: true } as const;
const str = { type: "string", required: true } as const;

export function registerCadActions(app: CadApp): void {
  // ── reads — safe to let the agent do unattended ──
  wrapAction(app.getModel.bind(app), {
    id: "get_model",
    description: "Get the current parametric model: dimension parameters and hole features.",
  });
  wrapAction(app.measure.bind(app), {
    id: "measure",
    description: "Compute the model's bounding box (mm) and solid volume (mm³) via the CAD kernel.",
  });
  wrapAction(app.exportStl.bind(app), {
    id: "export_stl",
    description: "Realize the current solid and export it to an STL file; returns the path and size.",
  });

  // ── routine edits — the everyday agent task ──
  wrapAction((name: string, value: number) => app.setParameter(name, value), {
    id: "set_parameter",
    description: "Set a named dimension parameter (width, height, thickness, cornerRadius) in millimetres.",
    parameters: { name: str, value: num },
    permissions: ["edit:parameter"],
    mapParams: (p) => [p.name, p.value],
  });
  wrapAction((x: number, y: number, diameter: number) => app.addHole(x, y, diameter), {
    id: "add_hole",
    description: "Add a through-hole at (x, y) mm from the plate centre, with the given diameter (mm).",
    parameters: { x: num, y: num, diameter: num },
    permissions: ["edit:feature"],
    mapParams: (p) => [p.x, p.y, p.diameter],
  });

  // ── destructive — gated by the policy (require_approval): an agent can propose, never decide ──
  wrapAction((id: string) => app.deleteFeature(id), {
    id: "delete_feature",
    description: "Delete a feature (hole) by its id.",
    parameters: { id: str },
    permissions: ["delete:feature"],
    mapParams: (p) => [p.id],
  });
  wrapAction(app.deleteBody.bind(app), {
    id: "delete_body",
    description: "Delete the model's solid body. Destroys the geometry.",
    permissions: ["delete:body"],
  });
  wrapAction(app.resetModel.bind(app), {
    id: "reset_model",
    description: "Reset the model to the default plate, discarding all edits.",
    permissions: ["reset:model"],
  });
}
