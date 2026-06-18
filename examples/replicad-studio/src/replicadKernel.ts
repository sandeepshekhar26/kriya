/**
 * Real geometry, in the browser. Replicad (OpenCascade compiled to WASM) builds the actual B-rep
 * solid — rounded plate, real boolean-cut holes — and returns a triangulated mesh + exact volume
 * and bounding box. This is Replicad's native habitat (the browser), so no Node shims are needed.
 */

import opencascade from "replicad-opencascadejs/src/replicad_single.js";
import opencascadeWasm from "replicad-opencascadejs/src/replicad_single.wasm?url";
import * as replicad from "replicad";
import type { CadModel } from "./cadModel";

let ocReady: Promise<void> | null = null;

function initOC(): Promise<void> {
  if (!ocReady) {
    ocReady = (async () => {
      const OC = await (opencascade as unknown as (opts: object) => Promise<unknown>)({
        locateFile: () => opencascadeWasm,
      });
      replicad.setOC(OC as never);
    })();
  }
  return ocReady;
}

export interface MeshData {
  positions: Float32Array;
  indices: Uint32Array;
  volumeMm3: number;
  bboxMm: { x: number; y: number; z: number };
}

const round2 = (n: number): number => Math.round(n * 100) / 100;

export async function buildMesh(model: CadModel): Promise<MeshData | null> {
  if (!model.hasBody) return null;
  await initOC();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const r: any = replicad;
  const { width, height, thickness, cornerRadius } = model.params;
  const radius = Math.max(0, Math.min(cornerRadius, Math.min(width, height) / 2 - 0.01));

  let solid = r.drawRoundedRectangle(width, height, radius).sketchOnPlane().extrude(thickness);
  for (const h of model.holes) {
    const drill = r.makeCylinder(h.diameter / 2, thickness + 2).translate([h.x, h.y, -1]);
    solid = solid.cut(drill);
  }

  const mesh = solid.mesh({ tolerance: 0.05, angularTolerance: 0.3 });
  const [min, max] = solid.boundingBox.bounds as [number[], number[]];

  return {
    positions: new Float32Array(mesh.vertices),
    indices: new Uint32Array(mesh.triangles),
    volumeMm3: round2(r.measureVolume(solid)),
    bboxMm: {
      x: round2((max[0] ?? 0) - (min[0] ?? 0)),
      y: round2((max[1] ?? 0) - (min[1] ?? 0)),
      z: round2((max[2] ?? 0) - (min[2] ?? 0)),
    },
  };
}
