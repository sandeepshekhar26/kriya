/**
 * The framework's dev inspector — what every agent-native app developer sees when
 * debugging an agent run. Filterable log, per-step expand, JSONL export.
 */

import { useMemo, useState } from "react";
import type { AgentLog } from "@agent-native/core";

export interface InspectorEntry extends AgentLog {
  ts: number;
}

const LEVELS = ["decision", "info", "warn", "error"] as const;
type Level = (typeof LEVELS)[number];

export interface AgentInspectorProps {
  log: InspectorEntry[];
  onClear?: () => void;
  /** Defaults to "agent-run.jsonl". */
  exportFilename?: string;
  /** Optional slot rendered below the controls (e.g. a MemoryPanel). */
  children?: React.ReactNode;
}

export function AgentInspector({
  log,
  onClear,
  exportFilename = "agent-run.jsonl",
  children,
}: AgentInspectorProps) {
  const [active, setActive] = useState<Set<Level>>(new Set(LEVELS));
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return log.filter((e) => {
      if (!active.has(e.level as Level)) return false;
      if (!q) return true;
      return (
        e.message.toLowerCase().includes(q) ||
        (e.detail != null && JSON.stringify(e.detail).toLowerCase().includes(q))
      );
    });
  }, [log, active, search]);

  const stepCount = useMemo(
    () => log.filter((e) => e.level === "decision").length,
    [log]
  );

  function toggle(level: Level) {
    setActive((prev) => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  }

  function exportJsonl() {
    const lines = log.map((e) => JSON.stringify(e)).join("\n");
    const blob = new Blob([lines + "\n"], { type: "application/x-ndjson" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = exportFilename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  return (
    <section className="an-inspector">
      <div className="an-inspector-head">
        <h2>agent inspector</h2>
        <div className="an-inspector-stats">
          <span className="an-step-count">{stepCount} actions</span>
          <button className="an-link" onClick={exportJsonl} disabled={log.length === 0}>
            export
          </button>
          {onClear && (
            <button className="an-link" onClick={onClear} disabled={log.length === 0}>
              clear
            </button>
          )}
        </div>
      </div>

      <div className="an-inspector-controls">
        <input
          className="an-search"
          placeholder="filter…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className="an-level-toggles">
          {LEVELS.map((l) => (
            <button
              key={l}
              className={`an-level-toggle an-level-${l} ${active.has(l) ? "on" : "off"}`}
              onClick={() => toggle(l)}
            >
              {l}
            </button>
          ))}
        </div>
      </div>

      <ol className="an-log">
        {filtered.length === 0 && (
          <li className="an-log-empty">
            {log.length === 0
              ? "Run the agent to watch it reason."
              : "No log entries match the current filter."}
          </li>
        )}
        {filtered.map((e, i) => (
          <LogRow key={i} entry={e} />
        ))}
      </ol>

      {children}
    </section>
  );
}

function LogRow({ entry }: { entry: InspectorEntry }) {
  const [expanded, setExpanded] = useState(false);
  const hasDetail = entry.detail != null;

  return (
    <li className={`an-log-entry an-level-${entry.level}`}>
      <button
        className="an-log-summary"
        onClick={() => hasDetail && setExpanded((v) => !v)}
        disabled={!hasDetail}
        type="button"
      >
        <span className="an-log-level">{entry.level}</span>
        <span className="an-log-msg">{entry.message}</span>
        {hasDetail && <span className="an-log-toggle">{expanded ? "▾" : "▸"}</span>}
      </button>
      {expanded && hasDetail && (
        <pre className="an-log-detail">{JSON.stringify(entry.detail, null, 2)}</pre>
      )}
    </li>
  );
}
