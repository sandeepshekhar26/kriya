/** The parametric model the studio renders and an agent edits — a mounting plate with holes. */

export interface CadParams {
  width: number;
  height: number;
  thickness: number;
  cornerRadius: number;
}

export interface Hole {
  id: string;
  x: number; // mm from plate centre
  y: number;
  diameter: number;
}

export interface CadModel {
  params: CadParams;
  holes: Hole[];
  hasBody: boolean;
}

export const PARAM_KEYS = ["width", "height", "thickness", "cornerRadius"] as const;
export type ParamKey = (typeof PARAM_KEYS)[number];

export function defaultModel(): CadModel {
  return {
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
