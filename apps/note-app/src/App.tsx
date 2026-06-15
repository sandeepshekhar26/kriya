import { useState } from "react";
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
  useAwaitStep,
  advanceStep,
  useRunCount,
} from "./agent";
import { AgentInspector, ApprovalModal, MemoryPanel, StepGate } from "kriya-inspector";
import "kriya-inspector/styles.css";

const ORGANIZE_GOAL =
  "Organize every note by assigning each one a single sensible category. " +
  "Categories you may use: work, shopping, personal, ideas. Only assign categories; " +
  "leave each note otherwise unchanged.";

const REMOVE_IDEAS_GOAL =
  "Delete every note in the ideas category. Remove each of them.";

export function App() {
  const notes = useNotes();
  const running = useAgentRunning();
  const pendingApproval = usePendingApproval();
  const log = useInspectorLog();
  const awaitStep = useAwaitStep();
  const runCount = useRunCount();
  const [stepMode, setStepMode] = useState(false);

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
          <label className="step-mode-toggle" title="Pause before each agent decision">
            <input
              type="checkbox"
              checked={stepMode}
              disabled={running}
              onChange={(e) => setStepMode(e.target.checked)}
            />
            step mode
          </label>
          <button onClick={seedNotes} disabled={running}>
            Seed 5 notes
          </button>
          <button
            className="primary"
            onClick={() => runAgentTask(ORGANIZE_GOAL, { stepMode })}
            disabled={running || notes.length === 0}
          >
            {running ? "Agent working…" : "Run agent: organize"}
          </button>
          <button
            onClick={() => runAgentTask(REMOVE_IDEAS_GOAL, { stepMode })}
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

        <AgentInspector log={log} onClear={clearLog} exportFilename="note-app-run.jsonl">
          <StepGate
            await_={awaitStep}
            onStep={() => advanceStep(true)}
            onStop={() => advanceStep(false)}
          />
          <MemoryPanel refreshKey={runCount} />
        </AgentInspector>
      </main>

      <ApprovalModal
        request={pendingApproval}
        onApprove={() => respondToApproval(true)}
        onDeny={() => respondToApproval(false)}
      />
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
