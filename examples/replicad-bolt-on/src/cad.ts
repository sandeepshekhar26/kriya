/**
 * The "app" we bolt onto: a tiny parametric-CAD document and the in-process functions it already
 * has (resize a dimension, drill a hole, measure, export, delete). In a real product these would
 * be the CAD app's existing functions; here they operate on a parametric mounting-plate model and
 * a pluggable geometry kernel (real Replicad/OpenCascade, or an analytic fallback).
 *
 * Note there is NOTHING about agents or governance in here — this is just the app. The bolt-on
 * (actions.ts) is what makes these functions agent-callable and governed.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import type { CadKernel, CadMetrics } from "./kernel.js";

export interface CadParams {
  width: number; // mm
  height: number; // mm
  thickness: number; // mm
  cornerRadius: number; // mm
}

export interface Hole {
  id: string;
  x: number; // mm from plate center
  y: number; // mm from plate center
  diameter: number; // mm
}

export interface CadModel {
  name: string;
  params: CadParams;
  holes: Hole[];
  hasBody: boolean;
}

export function defaultModel(): CadModel {
  return {
    name: "mounting-plate",
    params: { width: 80, height: 60, thickness: 4, cornerRadius: 6 },
    holes: [
      { id: "h1", x: -28, y: -18, diameter: 5 },
      { id: "h2", x: 28, y: -18, diameter: 5 },
      { id: "h3", x: -28, y: 18, diameter: 5 },
      { id: "h4", x: 28, y: 18, diameter: 5 },
    ],
    hasBody: true,
  };
}

const PARAM_KEYS = ["width", "height", "thickness", "cornerRadius"] as const;
type ParamKey = (typeof PARAM_KEYS)[number];

export class CadApp {
  private model: CadModel = defaultModel();
  private holeSeq = 5;

  constructor(private readonly kernel: CadKernel) {}

  /** Read: the current parametric model. */
  getModel(): CadModel {
    return structuredClone(this.model);
  }

  /** Read: bounding box + volume via the geometry kernel. */
  measure(): CadMetrics & { kernel: string } {
    if (!this.model.hasBody) throw new Error("no body in the model — it was deleted; nothing to measure.");
    return { ...this.kernel.measure(this.model), kernel: this.kernel.kind };
  }

  /** Read-ish: realize the solid and write an STL file. */
  async exportStl(): Promise<{ kernel: string; bytes: number; path: string }> {
    if (!this.model.hasBody) throw new Error("no body to export.");
    const stl = await this.kernel.exportStl(this.model);
    const path = join(tmpdir(), `kriya-cad-${this.model.name}.stl`);
    writeFileSync(path, stl);
    return { kernel: this.kernel.kind, bytes: Buffer.byteLength(stl), path };
  }

  /** Routine edit: change a named dimension. */
  setParameter(name: string, value: number): { params: CadParams } {
    if (!PARAM_KEYS.includes(name as ParamKey)) {
      throw new Error(`unknown parameter "${name}". Valid: ${PARAM_KEYS.join(", ")}.`);
    }
    if (!(value > 0)) throw new Error(`parameter "${name}" must be a positive number.`);
    this.model.params[name as ParamKey] = value;
    return { params: structuredClone(this.model.params) };
  }

  /** Routine edit: add a through-hole. */
  addHole(x: number, y: number, diameter: number): { added: string; holes: number } {
    if (!this.model.hasBody) throw new Error("no body to add a hole to.");
    if (!(diameter > 0)) throw new Error("diameter must be a positive number.");
    const id = `h${this.holeSeq++}`;
    this.model.holes.push({ id, x, y, diameter });
    return { added: id, holes: this.model.holes.length };
  }

  /** Destructive: remove a feature. */
  deleteFeature(id: string): { deleted: string; holes: number } {
    const i = this.model.holes.findIndex((h) => h.id === id);
    if (i < 0) throw new Error(`no such feature "${id}".`);
    this.model.holes.splice(i, 1);
    return { deleted: id, holes: this.model.holes.length };
  }

  /** Destructive: delete the solid body. */
  deleteBody(): { hasBody: boolean } {
    if (!this.model.hasBody) throw new Error("body already deleted.");
    this.model.hasBody = false;
    this.model.holes = [];
    return { hasBody: false };
  }

  /** Destructive: discard all edits. */
  resetModel(): { reset: boolean; params: CadParams } {
    this.model = defaultModel();
    this.holeSeq = 5;
    return { reset: true, params: structuredClone(this.model.params) };
  }
}
