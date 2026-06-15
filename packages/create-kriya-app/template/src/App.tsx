import { useMemo, useState } from "react";
import { increment, resetCounter } from "./actions";
import { useCount } from "./store";
import {
  runAgentTask,
  useAgentRunning,
  useInspectorLog,
  clearLog,
  usePendingApproval,
  respondToApproval,
} from "./agent";

const DEFAULT_GOAL = "Make the counter equal to 5.";

export function App() {
  const count = useCount();
  const running = useAgentRunning();
  const [goal, setGoal] = useState(DEFAULT_GOAL);

  return (
    <div className="app">
      <header>
        <div>
          <h1>{{TITLE}}</h1>
          <p className="tagline">
            one app · two users — humans click, the agent calls typed actions
          </p>
        </div>
      </header>

      <main>
        <section className="counter">
          <div className="count-display">
            <span className="count-value">{count}</span>
          </div>
          <div className="counter-buttons">
            <button onClick={() => increment.call({})} disabled={running}>
              +1
            </button>
            <button
              onClick={() => resetCounter.call({})}
              disabled={running || count === 0}
              title="Destructive — guarded by policy when the agent calls it"
            >
              Reset
            </button>
          </div>

          <form
            className="goal-form"
            onSubmit={(e) => {
              e.preventDefault();
              if (!running && goal.trim()) runAgentTask(goal.trim());
            }}
          >
            <label htmlFor="goal">Agent goal</label>
            <input
              id="goal"
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
              disabled={running}
            />
            <button type="submit" className="primary" disabled={running || !goal.trim()}>
              {running ? "Agent working…" : "Run agent"}
            </button>
          </form>
        </section>

        <Inspector />
      </main>

      <ApprovalModal />
    </div>
  );
}

function ApprovalModal() {
  const req = usePendingApproval();
  if (!req) return null;
  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h3>Approval required</h3>
        <p className="modal-sub">
          The agent wants to run a guarded action. The host is paused until you decide.
        </p>
        <div className="modal-action">
          <code className="modal-action-id">{req.actionId}</code>
          <pre>{JSON.stringify(req.params, null, 2)}</pre>
          {req.reasoning && <p className="modal-reason">“{req.reasoning}”</p>}
        </div>
        <div className="modal-buttons">
          <button onClick={() => respondToApproval(false)}>Deny</button>
          <button className="primary danger" onClick={() => respondToApproval(true)}>
            Approve
          </button>
        </div>
      </div>
    </div>
  );
}

function Inspector() {
  const log = useInspectorLog();
  const stepCount = useMemo(() => log.filter((e) => e.level === "decision").length, [log]);
  return (
    <section className="inspector">
      <div className="inspector-head">
        <h2>agent inspector</h2>
        <div>
          <span className="step-count">{stepCount} actions</span>
          <button className="link" onClick={clearLog}>
            clear
          </button>
        </div>
      </div>
      <ol className="log">
        {log.length === 0 && <li className="log-empty">Run the agent to watch it reason.</li>}
        {log.map((e, i) => (
          <li key={i} className={`log-entry level-${e.level}`}>
            <span className="log-level">{e.level}</span>
            <span className="log-msg">{e.message}</span>
            {e.detail != null && (
              <pre className="log-detail">{JSON.stringify(e.detail, null, 2)}</pre>
            )}
          </li>
        ))}
      </ol>
    </section>
  );
}
