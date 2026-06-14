/**
 * The developer-controlled gate for step-mode runs. When the host pauses before
 * a step (`AgentAwaitStep` event), the app surfaces that payload here; the
 * developer either advances to the next step or stops the run.
 *
 * Keyboard: Space = step, Esc = stop. Both ignore typing in text inputs so
 * filtering in the inspector still works.
 */

import { useEffect } from "react";
import type { AgentAwaitStep } from "@agent-native/core";

export interface StepGateProps {
  await_: AgentAwaitStep | null;
  onStep: () => void;
  onStop: () => void;
}

export function StepGate({ await_, onStep, onStop }: StepGateProps) {
  useEffect(() => {
    if (!await_) return;
    function isTextInput(target: EventTarget | null): boolean {
      if (!(target instanceof HTMLElement)) return false;
      const tag = target.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
      return target.isContentEditable;
    }
    function onKey(e: KeyboardEvent) {
      if (isTextInput(e.target)) return;
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        onStep();
      } else if (e.key === "Escape") {
        e.preventDefault();
        onStop();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [await_, onStep, onStop]);

  if (!await_) return null;

  return (
    <section className="an-step-gate" role="status" aria-live="polite">
      <div className="an-step-gate-head">
        <span className="an-step-gate-badge">paused</span>
        <span className="an-step-gate-title">step {await_.stepNumber}</span>
      </div>
      {await_.lastActionId ? (
        <p className="an-step-gate-last">
          last: <code>{await_.lastActionId}</code> —{" "}
          <span className={await_.lastSuccess ? "an-ok" : "an-fail"}>
            {await_.lastSuccess ? "ok" : "failed"}
          </span>
        </p>
      ) : (
        <p className="an-step-gate-last an-step-gate-first">
          first step — nothing executed yet.
        </p>
      )}
      <div className="an-step-gate-buttons">
        <button className="an-link" onClick={onStop} title="Stop the run (Esc)">
          stop
        </button>
        <button
          className="an-primary"
          onClick={onStep}
          autoFocus
          title="Advance to the next step (Space / Enter)"
        >
          step →
        </button>
      </div>
    </section>
  );
}
