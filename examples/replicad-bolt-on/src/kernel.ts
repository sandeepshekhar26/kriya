/**
 * Pluggable geometry kernel. The real one is **Replicad** (OpenCascade compiled to WASM): it
 * builds the actual B-rep solid, drills real holes with boolean cuts, and reports exact volume +
 * bounding box and a real STL. The analytic fallback computes the same metrics in closed form (no
 * WASM) so the demo runs anywhere / in CI (`CAD_FAKE=1`). Governance is identical either way.
 */

import type { CadModel } from "./cad.js";

export interface CadMetrics {
  boundingBoxMm: { x: number; y: number; z: number };
  volumeMm3: number;
  holes: number;
}

export interface CadKernel {
  readonly kind: "replicad" | "analytic";
  measure(model: CadModel): CadMetrics;
  exportStl(model: CadModel): Promise<string>;
}

const round2 = (n: number): number => Math.round(n * 100) / 100;

// ── analytic fallback ─────────────────────────────────────────────────────────
export function analyticKernel(): CadKernel {
  return {
    kind: "analytic",
    measure(model) {
      const { width, height, thickness } = model.params;
      const holesVol = model.holes.reduce((s, h) => s + Math.PI * (h.diameter / 2) ** 2 * thickness, 0);
      // Exact for a rectangular plate minus cylindrical holes; ignores corner-radius rounding
      // (the Replicad kernel captures that exactly).
      return {
        boundingBoxMm: { x: width, y: height, z: thickness },
        volumeMm3: round2(Math.max(0, width * height * thickness - holesVol)),
        holes: model.holes.length,
      };
    },
    async exportStl(model) {
      const { width, height, thickness } = model.params;
      return boxStl(width, height, thickness);
    },
  };
}

// ── real Replicad / OpenCascade kernel ──────────────────────────────────────────
export async function loadReplicadKernel(): Promise<CadKernel> {
  const { createRequire } = await import("node:module");
  const { fileURLToPath } = await import("node:url");
  const { readFileSync } = await import("node:fs");
  const require = createRequire(import.meta.url);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const replicad: any = await import("replicad");

  const ocSrcDir = fileURLToPath(new URL("../node_modules/replicad-opencascadejs/src/", import.meta.url));
  // The OpenCascade Emscripten glue references a free `__dirname`, which doesn't exist under ESM.
  // Shim it (we hand the factory the wasm bytes directly, so the value itself is unused).
  const g = globalThis as unknown as { __dirname?: string; __filename?: string };
  g.__dirname ??= ocSrcDir;
  g.__filename ??= ocSrcDir + "replicad_single.js";

  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const ocMod = require("replicad-opencascadejs/src/replicad_single.js");
  const ocFactory = typeof ocMod === "function" ? ocMod : ocMod.default;
  const wasmPath = ocSrcDir + "replicad_single.wasm";
  const OC = await ocFactory({ wasmBinary: readFileSync(wasmPath), locateFile: () => wasmPath });
  replicad.setOC(OC);

  function build(model: CadModel): unknown {
    const { width, height, thickness, cornerRadius } = model.params;
    const r = Math.max(0, Math.min(cornerRadius, Math.min(width, height) / 2 - 0.01));
    let solid = replicad.drawRoundedRectangle(width, height, r).sketchOnPlane().extrude(thickness);
    for (const h of model.holes) {
      const drill = replicad.makeCylinder(h.diameter / 2, thickness + 2).translate([h.x, h.y, -1]);
      solid = solid.cut(drill);
    }
    return solid;
  }

  return {
    kind: "replicad",
    measure(model) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const solid: any = build(model);
      const [min, max] = solid.boundingBox.bounds as [number[], number[]];
      return {
        boundingBoxMm: {
          x: round2(max[0]! - min[0]!),
          y: round2(max[1]! - min[1]!),
          z: round2(max[2]! - min[2]!),
        },
        volumeMm3: round2(replicad.measureVolume(solid)),
        holes: model.holes.length,
      };
    },
    async exportStl(model) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const solid: any = build(model);
      const blob = solid.blobSTL();
      return Buffer.from(await blob.arrayBuffer()).toString("utf8");
    },
  };
}

// A minimal valid ASCII STL of the bounding box — used only by the analytic fallback.
function boxStl(w: number, h: number, t: number): string {
  const x0 = -w / 2, x1 = w / 2, y0 = -h / 2, y1 = h / 2, z0 = 0, z1 = t;
  const P: number[][] = [
    [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
    [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
  ];
  const quads: { n: [number, number, number]; idx: [number, number, number, number] }[] = [
    { n: [0, 0, -1], idx: [0, 3, 2, 1] },
    { n: [0, 0, 1], idx: [4, 5, 6, 7] },
    { n: [0, -1, 0], idx: [0, 1, 5, 4] },
    { n: [1, 0, 0], idx: [1, 2, 6, 5] },
    { n: [0, 1, 0], idx: [2, 3, 7, 6] },
    { n: [-1, 0, 0], idx: [3, 0, 4, 7] },
  ];
  let s = "solid kriya_plate\n";
  for (const { n, idx } of quads) {
    for (const tri of [[idx[0], idx[1], idx[2]], [idx[0], idx[2], idx[3]]]) {
      s += ` facet normal ${n[0]} ${n[1]} ${n[2]}\n  outer loop\n`;
      for (const i of tri) {
        const p = P[i]!;
        s += `   vertex ${p[0]} ${p[1]} ${p[2]}\n`;
      }
      s += "  endloop\n endfacet\n";
    }
  }
  return s + "endsolid kriya_plate\n";
}
