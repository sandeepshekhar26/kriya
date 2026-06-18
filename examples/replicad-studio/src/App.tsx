import { useEffect, useRef, useState } from "react";
import { Viewport } from "./Viewport";
import { buildMesh, type MeshData } from "./replicadKernel";
import { defaultModel, PARAM_KEYS, type CadModel } from "./cadModel";
import { applyAction, decide, TIER_LABEL, type Tier } from "./governance";

interface AuditEntry {
  n: number;
  id: string;
  decision: Tier;
  ok: boolean;
  detail: string;
}
interface Pending {
  id: string;
  params: Record<string, unknown>;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function App() {
  const [model, setModelState] = useState<CadModel>(defaultModel);
  const modelRef = useRef(model);
  const setModel = (m: CadModel) => {
    modelRef.current = m;
    setModelState(m);
  };

  const [mesh, setMesh] = useState<MeshData | null>(null);
  const meshRef = useRef<MeshData | null>(null);
  const [status, setStatus] = useState("loading geometry kernel…");
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [pending, setPending] = useState<Pending | null>(null);
  const [agentRunning, setAgentRunning] = useState(false);
  const resolverRef = useRef<((ok: boolean) => void) | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!model.hasBody) {
      setMesh(null);
      meshRef.current = null;
      setStatus("body deleted — no geometry");
      return;
    }
    buildMesh(model)
      .then((m) => {
        if (!cancelled) {
          setMesh(m);
          meshRef.current = m;
          setStatus("");
        }
      })
      .catch((e) => {
        if (!cancelled) setStatus("kernel error: " + (e instanceof Error ? e.message : String(e)));
      });
    return () => {
      cancelled = true;
    };
  }, [model]);

  function pushAudit(id: string, decision: Tier, ok: boolean, detail: string) {
    setAudit((prev) => [...prev, { n: prev.length + 1, id, decision, ok, detail }]);
  }

  async function runAction(id: string, params: Record<string, unknown>): Promise<void> {
    const tier = decide(id);
    if (tier === "deny") {
      pushAudit(id, "deny", false, "denied by policy");
      return;
    }
    if (tier === "approval") {
      const approved = await new Promise<boolean>((res) => {
        resolverRef.current = res;
        setPending({ id, params });
      });
      setPending(null);
      resolverRef.current = null;
      if (!approved) {
        pushAudit(id, "deny", false, "approval denied by operator");
        return;
      }
    }
    if (id === "measure") {
      const m = meshRef.current;
      pushAudit(
        "measure",
        "allow",
        true,
        m ? `bbox ${m.bboxMm.x}×${m.bboxMm.y}×${m.bboxMm.z} mm · vol ${m.volumeMm3} mm³` : "no body",
      );
      return;
    }
    try {
      const next = structuredClone(modelRef.current);
      const summary = applyAction(next, id, params);
      setModel(next);
      pushAudit(id, tier, true, summary);
    } catch (e) {
      pushAudit(id, tier, false, e instanceof Error ? e.message : String(e));
    }
  }

  function resolveApproval(ok: boolean) {
    resolverRef.current?.(ok);
  }

  async function runAgentDemo() {
    if (agentRunning) return;
    setAgentRunning(true);
    try {
      const seq: [string, Record<string, unknown>][] = [
        ["measure", {}],
        ["set_parameter", { name: "width", value: 110 }],
        ["set_parameter", { name: "thickness", value: 6 }],
        ["add_hole", { x: 0, y: 0, diameter: 10 }],
        ["measure", {}],
        ["delete_body", {}],
      ];
      for (const [id, p] of seq) {
        await runAction(id, p);
        await sleep(950);
      }
    } finally {
      setAgentRunning(false);
    }
  }

  const params = model.params;

  return (
    <div className="studio">
      <header className="topbar">
        <div className="brand">
          <span className="logo">▣</span>
          <div>
            <div className="title">kriya CAD Studio</div>
            <div className="sub">agent-driven parametric CAD · governed in-process</div>
          </div>
        </div>
        <button className="btn agent" disabled={agentRunning} onClick={() => void runAgentDemo()}>
          {agentRunning ? "agent running…" : "▶ Run agent demo"}
        </button>
      </header>

      <div className="body">
        <Viewport mesh={mesh} status={status} />

        <aside className="sidebar">
          <section className="card">
            <h3>Model</h3>
            <div className="stat-row"><span>Volume</span><b>{mesh ? `${mesh.volumeMm3} mm³` : "—"}</b></div>
            <div className="stat-row"><span>Bounding box</span><b>{mesh ? `${mesh.bboxMm.x}×${mesh.bboxMm.y}×${mesh.bboxMm.z}` : "—"}</b></div>
            <div className="stat-row"><span>Holes</span><b>{model.hasBody ? model.holes.length : "—"}</b></div>
            <div className="stat-row"><span>Kernel</span><b className="ok">Replicad · OpenCascade</b></div>
          </section>

          <section className="card">
            <h3>Parameters <span className="muted small">agent: set_parameter</span></h3>
            {PARAM_KEYS.map((k) => (
              <label className="param" key={k}>
                <span>{k}</span>
                <input
                  type="number"
                  min={0.5}
                  step={1}
                  value={params[k]}
                  disabled={!model.hasBody}
                  onChange={(e) => void runAction("set_parameter", { name: k, value: Number(e.target.value) })}
                />
              </label>
            ))}
          </section>

          <section className="card">
            <h3>Actions</h3>
            <div className="actions">
              <button className="btn ghost" disabled={!model.hasBody} onClick={() => void runAction("add_hole", { x: 0, y: 0, diameter: 8 })}>
                Drill centre hole
              </button>
              <button className="btn ghost" disabled={!model.hasBody || model.holes.length === 0} onClick={() => void runAction("delete_feature", { id: model.holes[0]?.id })}>
                Delete a hole ⏸
              </button>
              <button className="btn ghost danger" disabled={!model.hasBody} onClick={() => void runAction("delete_body", {})}>
                Delete body ⏸
              </button>
              <button className="btn ghost" onClick={() => void runAction("reset_model", {})}>
                Reset model ⏸
              </button>
            </div>
            <p className="muted small">⏸ destructive → requires on-device approval</p>
          </section>

          <section className="card grow">
            <h3>Audit <span className="muted small">{audit.length}</span></h3>
            <div className="audit">
              {audit.length === 0 && <p className="muted small">No actions yet — hit “Run agent demo”.</p>}
              {audit
                .slice()
                .reverse()
                .map((a) => (
                  <div className="audit-row" key={a.n}>
                    <span className={`badge ${a.ok ? a.decision : "blocked"}`}>{a.ok ? TIER_LABEL[a.decision] : "blocked"}</span>
                    <span className="mono">{a.id}</span>
                    <span className="detail">{a.detail}</span>
                  </div>
                ))}
            </div>
          </section>
        </aside>
      </div>

      {pending && (
        <div className="modal-overlay">
          <div className="modal">
            <div className="modal-title">kriya — approval required</div>
            <p className="modal-body">An agent wants to run a destructive action:</p>
            <div className="modal-action">
              <b>{pending.id}</b> <span className="mono">{JSON.stringify(pending.params)}</span>
            </div>
            <p className="muted small">This destroys geometry and cannot be auto-approved. You decide.</p>
            <div className="modal-buttons">
              <button className="btn ghost" onClick={() => resolveApproval(false)}>Deny</button>
              <button className="btn" onClick={() => resolveApproval(true)}>Approve</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
