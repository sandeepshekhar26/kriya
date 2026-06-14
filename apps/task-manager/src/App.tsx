import { useMemo, useState } from "react";
import { completeTask, createTask, deleteTask, setPriority } from "./actions";
import { useTasks, type Priority, type Task } from "./store";
import { seedTasks } from "./seed";
import {
  runAgentTask,
  useAgentRunning,
  useInspectorLog,
  clearLog,
  usePendingApproval,
  respondToApproval,
} from "./agent";

const FINISH_HIGH_GOAL =
  "Complete every high-priority task. Mark each high-priority task as done; " +
  "leave medium and low priority tasks alone.";

const CLEAR_DONE_GOAL =
  "Delete every completed task. Remove each task that is already done.";

export function App() {
  const tasks = useTasks();
  const running = useAgentRunning();

  return (
    <div className="app">
      <header>
        <div>
          <h1>tasks</h1>
          <p className="tagline">
            one app · two users — humans click, the agent calls typed actions
          </p>
        </div>
        <div className="header-actions">
          <button onClick={seedTasks} disabled={running}>
            Seed 6 tasks
          </button>
          <button
            className="primary"
            onClick={() => runAgentTask(FINISH_HIGH_GOAL)}
            disabled={running || tasks.length === 0}
          >
            {running ? "Agent working…" : "Run agent: finish high-priority"}
          </button>
          <button
            onClick={() => runAgentTask(CLEAR_DONE_GOAL)}
            disabled={running || tasks.length === 0}
            title="Agent will propose deletes, which require your approval"
          >
            Run agent: delete completed
          </button>
        </div>
      </header>

      <main>
        <section className="tasks">
          <NewTaskForm disabled={running} />
          {tasks.length === 0 ? (
            <p className="empty">No tasks yet — seed some or add one above.</p>
          ) : (
            <ul className="task-list">
              {tasks.map((t) => (
                <TaskRow key={t.id} task={t} disabled={running} />
              ))}
            </ul>
          )}
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

function NewTaskForm({ disabled }: { disabled: boolean }) {
  const [title, setTitle] = useState("");
  const [priority, setPriorityValue] = useState<Priority>("medium");
  return (
    <form
      className="new-task"
      onSubmit={(e) => {
        e.preventDefault();
        if (!title.trim()) return;
        createTask.call({ title: title.trim(), priority });
        setTitle("");
      }}
    >
      <input
        placeholder="What needs doing?"
        value={title}
        disabled={disabled}
        onChange={(e) => setTitle(e.target.value)}
      />
      <select
        value={priority}
        disabled={disabled}
        onChange={(e) => setPriorityValue(e.target.value as Priority)}
      >
        <option value="low">low</option>
        <option value="medium">medium</option>
        <option value="high">high</option>
      </select>
      <button type="submit" disabled={disabled}>
        Add
      </button>
    </form>
  );
}

function TaskRow({ task, disabled }: { task: Task; disabled: boolean }) {
  return (
    <li className={`task-row ${task.done ? "done" : ""}`}>
      <input
        type="checkbox"
        checked={task.done}
        disabled={disabled || task.done}
        onChange={() => {
          if (!task.done) completeTask.call({ id: task.id });
        }}
      />
      <span className="task-title">{task.title}</span>
      <select
        className={`badge prio-${task.priority}`}
        value={task.priority}
        disabled={disabled}
        onChange={(e) =>
          setPriority.call({ id: task.id, priority: e.target.value as Priority })
        }
      >
        <option value="low">low</option>
        <option value="medium">medium</option>
        <option value="high">high</option>
      </select>
      <button
        className="link"
        disabled={disabled}
        onClick={() => deleteTask.call({ id: task.id })}
        title="Delete (no approval needed for human-initiated deletes)"
      >
        delete
      </button>
    </li>
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
