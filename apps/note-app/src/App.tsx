import { useMemo, useState } from "react";
import { createNote } from "./actions";
import { useNotes, type Note } from "./store";
import { seedNotes } from "./seed";
import {
  runAgentTask,
  useAgentRunning,
  useInspectorLog,
  clearLog,
  usePendingApproval,
  respondToApproval,
} from "./agent";

const ORGANIZE_GOAL =
  "Organize every note by assigning each one a single sensible category. " +
  "Categories you may use: work, shopping, personal, ideas. Only assign categories; " +
  "leave each note otherwise unchanged.";

const REMOVE_IDEAS_GOAL =
  "Delete every note in the ideas category. Remove each of them.";

export function App() {
  const notes = useNotes();
  const running = useAgentRunning();

  return (
    <div className="app">
      <header>
        <div>
          <h1>notes</h1>
          <p className="tagline">
            one app · two users — humans click, the agent calls typed actions
          </p>
        </div>
        <div className="header-actions">
          <button onClick={seedNotes} disabled={running}>
            Seed 5 notes
          </button>
          <button
            className="primary"
            onClick={() => runAgentTask(ORGANIZE_GOAL)}
            disabled={running || notes.length === 0}
          >
            {running ? "Agent working…" : "Run agent: organize"}
          </button>
          <button
            onClick={() => runAgentTask(REMOVE_IDEAS_GOAL)}
            disabled={running || notes.length === 0}
            title="Agent will propose deletes, which require your approval"
          >
            Run agent: remove ideas
          </button>
        </div>
      </header>

      <main>
        <section className="notes">
          <NewNoteForm disabled={running} />
          {notes.length === 0 ? (
            <p className="empty">No notes yet — seed some or add one above.</p>
          ) : (
            <ul className="note-list">
              {notes.map((n) => (
                <NoteCard key={n.id} note={n} />
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

function NewNoteForm({ disabled }: { disabled: boolean }) {
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  return (
    <form
      className="new-note"
      onSubmit={(e) => {
        e.preventDefault();
        if (!title.trim()) return;
        createNote.call({ title: title.trim(), content: content.trim() });
        setTitle("");
        setContent("");
      }}
    >
      <input
        placeholder="Note title"
        value={title}
        disabled={disabled}
        onChange={(e) => setTitle(e.target.value)}
      />
      <input
        placeholder="Content (optional)"
        value={content}
        disabled={disabled}
        onChange={(e) => setContent(e.target.value)}
      />
      <button type="submit" disabled={disabled}>
        Add
      </button>
    </form>
  );
}

function NoteCard({ note }: { note: Note }) {
  return (
    <li className="note-card">
      <div className="note-head">
        <span className="note-title">{note.title}</span>
        {note.category ? (
          <span className={`badge cat-${note.category}`}>{note.category}</span>
        ) : (
          <span className="badge uncategorized">uncategorized</span>
        )}
      </div>
      {note.content && <p className="note-content">{note.content}</p>}
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
