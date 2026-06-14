/**
 * Past-runs viewer + step-through replay. Queries the durable episodic memory the
 * Rust host writes to (every action across runs) via the `agent_memory_recent`
 * Tauri command and renders a compact list — newest first. Click an item to open
 * it; use Prev/Next (buttons or ←/→ keys) to step through neighbouring episodes
 * one at a time. This is the "replay" surface that complements the live inspector.
 *
 * The host exposes this command in every agent-native app's Tauri backend; this
 * component is wired up the same way regardless of app.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface MemoryEpisode {
  id: number;
  ts_ms: number;
  action_id: string;
  params: unknown;
  success: boolean;
  reasoning: string;
  signature: string;
}

export interface MemoryPanelProps {
  /** Defaults to 20. */
  limit?: number;
  /** Override the command name if the host renamed it. */
  command?: string;
  /** Auto-refresh after a run; pass a counter that bumps each completion. */
  refreshKey?: number;
}

export function MemoryPanel({
  limit = 20,
  command = "agent_memory_recent",
  refreshKey = 0,
}: MemoryPanelProps) {
  const [episodes, setEpisodes] = useState<MemoryEpisode[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [openId, setOpenId] = useState<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = (await invoke(command, { limit })) as MemoryEpisode[];
      setEpisodes(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [command, limit]);

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const openIndex = useMemo(() => {
    if (openId == null) return -1;
    return episodes.findIndex((e) => e.id === openId);
  }, [episodes, openId]);

  const step = useCallback(
    (delta: 1 | -1) => {
      if (openIndex < 0 || episodes.length === 0) return;
      const next = openIndex + delta;
      if (next < 0 || next >= episodes.length) return;
      setOpenId(episodes[next]!.id);
    },
    [episodes, openIndex]
  );

  // Keyboard nav while an episode is open. Ignore the keys when focus is in a
  // text input so devs can still type into the inspector's filter box.
  useEffect(() => {
    if (openId == null) return;
    function isTextInput(target: EventTarget | null): boolean {
      if (!(target instanceof HTMLElement)) return false;
      const tag = target.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
      return target.isContentEditable;
    }
    function onKey(e: KeyboardEvent) {
      if (isTextInput(e.target)) return;
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        step(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        step(1);
      } else if (e.key === "Escape") {
        setOpenId(null);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openId, step]);

  return (
    <section className="an-memory">
      <div className="an-memory-head">
        <h3>past runs</h3>
        <button className="an-link" onClick={() => void load()} disabled={loading}>
          {loading ? "loading…" : "refresh"}
        </button>
      </div>

      {error && <p className="an-memory-error">{error}</p>}

      {!error && episodes.length === 0 && !loading && (
        <p className="an-memory-empty">No past actions recorded yet.</p>
      )}

      <ol className="an-memory-list">
        {episodes.map((ep, i) => (
          <li key={ep.id} className={`an-memory-item ${ep.success ? "ok" : "fail"}`}>
            <button
              className="an-memory-summary"
              onClick={() => setOpenId((id) => (id === ep.id ? null : ep.id))}
              type="button"
            >
              <span className="an-memory-action">{ep.action_id}</span>
              <span className="an-memory-status">{ep.success ? "ok" : "fail"}</span>
              <span className="an-memory-ts">{formatTime(ep.ts_ms)}</span>
            </button>
            {openId === ep.id && (
              <div className="an-memory-detail">
                {ep.reasoning && <p className="an-memory-reason">“{ep.reasoning}”</p>}
                <pre>{JSON.stringify(ep.params, null, 2)}</pre>
                <code className="an-memory-sig">sig {ep.signature.slice(0, 24)}…</code>

                <div className="an-memory-nav" role="group" aria-label="Replay navigation">
                  <button
                    className="an-link"
                    onClick={() => step(-1)}
                    disabled={i === 0}
                    title="Previous episode (← key)"
                    type="button"
                  >
                    ← prev
                  </button>
                  <span className="an-memory-position">
                    {i + 1} / {episodes.length}
                  </span>
                  <button
                    className="an-link"
                    onClick={() => step(1)}
                    disabled={i === episodes.length - 1}
                    title="Next episode (→ key)"
                    type="button"
                  >
                    next →
                  </button>
                </div>
              </div>
            )}
          </li>
        ))}
      </ol>
    </section>
  );
}

function formatTime(ts_ms: number): string {
  const d = new Date(ts_ms);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
