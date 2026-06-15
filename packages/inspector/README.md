# @agent-native/inspector

Drop-in React inspector for [**verb**](https://github.com/sandeepshekhar26/verb) apps — watch an
agent drive your app, approve guarded actions, and replay past runs. Same look-and-feel across
every app on the framework: every developer who picks up `@agent-native/core` gets the same
debugging surface.

What's in the box:

- **`<AgentInspector>`** — filterable step log (toggle by level, full-text search),
  collapsible per-step detail, one-click JSONL export of the current run.
- **`<ApprovalModal>`** — the human-in-the-loop dialog that fires when the Rust host
  pauses on a guarded action. Drop in, pass `{ request, onApprove, onDeny }`.
- **`<MemoryPanel>`** — past runs pulled from the host's durable SQLite memory via the
  `agent_memory_recent` Tauri command. Shows action id, ok/fail, timestamp, reasoning,
  signed receipt prefix. **Step-through replay**: click an episode to open it, then use
  Prev/Next buttons or ←/→ keys to walk through neighbouring episodes; Esc closes.

## Usage

```tsx
import {
  AgentInspector,
  ApprovalModal,
  MemoryPanel,
} from "@agent-native/inspector";
import "@agent-native/inspector/styles.css";

<AgentInspector log={log} onClear={clearLog}>
  <MemoryPanel refreshKey={runCount} />
</AgentInspector>

<ApprovalModal
  request={pending}
  onApprove={() => respondToApproval(true)}
  onDeny={() => respondToApproval(false)}
/>
```

Theming: override the CSS variables on a parent (e.g. `--an-inspector-accent`,
`--an-inspector-bg`, `--an-inspector-danger`) to match your app's palette.

Peer-deps: `@agent-native/core`, `@tauri-apps/api`, `react`.
