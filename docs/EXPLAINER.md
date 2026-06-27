# kriya — Plain-English Explainer

A cheat sheet for talking about this project. Audience: technical people who aren't
necessarily AI/desktop experts. Keep it simple, use the analogies.

> **⚠️ PARTIALLY UPDATED — the front door changed (D-016, D-018).** The deepest sections below
> still describe the original "kriya desktop framework" pitch. The product you actually download
> is sharper: *an **on-device control plane** — one Mac app that puts a governance checkpoint in
> front of any MCP agent (Claude Desktop, Cursor, …), with nothing leaving your machine.* The
> governance substance (deny-by-default → approval → budget → signed, verifiable audit) is
> unchanged and correct; only the **delivery model** moved — from a framework you build on to an
> app you install. Sections 1, 6 and 7 carry the current control-plane / freemium framing; see
> [strategy/governed-local-first-wedge.md](strategy/governed-local-first-wedge.md) for the
> defensibility-vs-MCP story.

---

## 1. The one-liner

> **"It's the on-device control plane for AI agents on your Mac. You download one app: it
> installs a governance gateway, walks you through the macOS permissions, wires up your MCP
> client, and shows you live, signed proof of everything an agent does. Nothing leaves your
> machine."**

Think: *"a governance checkpoint you drop in front of any MCP agent — zero integration code."*

---

## 2. The problem (why this exists)

Every app today was built for **humans clicking buttons**. Now AI agents need to use those
same apps. Today they do it the hard way:

- Take a **screenshot** of the screen
- Run a **vision model** to guess what's on it
- Guess **where to click**
- Hope the layout didn't change

This is slow, expensive, and breaks the moment you redesign a button. It's like teaching a
robot to use your computer by holding a camera up to the monitor.

**Our idea:** instead of the agent watching pixels, the app **tells the agent what it can do**
— as a clean list of typed commands ("actions"). The agent calls those commands directly. No
screenshots, no guessing.

**The mental model:** one app, two doors in.
- **Humans** come in through the **buttons** (the visual UI).
- **Agents** come in through the **actions** (typed commands).
- Both doors lead to the *same* code underneath.

---

## 3. How it actually works (the loop)

We built a real demo app: a **note-taking app**. You click "Run agent: organize," and a local
AI sorts all your notes into categories by itself. Here's the loop, in plain terms:

1. The app shows the agent its **state** (the list of notes) as plain data, and a **menu of
   actions** it's allowed to call (`create_note`, `edit_note`, `delete_note`).
2. The agent picks one action and says "call `edit_note` with category = work."
3. A **gatekeeper** (our Rust "agent host") checks: *is this allowed? does it need human
   approval? are we over the rate limit?*
4. If OK, the app runs that command — the **same** command a human button would trigger — and
   the screen updates.
5. Every action gets a **tamper-proof receipt** (a cryptographic signature) saved to a log.
6. Repeat until the agent says "done."

No vision. No clicking. Just structured data and typed commands.

---

## 4. The tech stack — what each piece is and where it fits

Picture three layers stacked on top of each other:

```
┌─────────────────────────────────────────────────────────┐
│  THE WINDOW & UI         React + TypeScript (in Tauri)   │  ← what the human sees
├─────────────────────────────────────────────────────────┤
│  THE BRAIN / GATEKEEPER  Rust ("agent host")             │  ← decides & enforces rules
├─────────────────────────────────────────────────────────┤
│  THE AI                  local model / Claude / Ollama   │  ← picks the next action
└─────────────────────────────────────────────────────────┘
```

| Tech | What it is (plain terms) | Where it fits / why we use it |
|---|---|---|
| **Tauri** | A toolkit for building desktop apps using web tech, but lightweight (~10MB vs Electron's ~300MB). | The **app shell** — the actual window the user opens. It uses the OS's built-in browser instead of bundling a whole Chrome. |
| **React + TypeScript** | The standard way to build web/app interfaces; TypeScript = JavaScript with type-checking so bugs get caught early. | The **visual UI** (notes, buttons, the agent inspector panel) and the small **SDK** developers use to declare actions. |
| **Rust** | A fast, memory-safe systems language. No crashes from memory bugs; great for security-sensitive code. | The **agent host** — the gatekeeper that runs the agent loop, checks permissions, enforces limits, and signs the audit trail. The trustworthy core. |
| **The "action registry"** | A simple function — `registerAction(...)` — where a developer declares what their app can do (name, description, parameters). | This is the **heart of the framework**. Declare once; humans and agents both use it. It auto-generates a machine-readable menu for the AI. |
| **JSON over IPC** | Plain text messages (JSON) passed between the UI and the Rust core. IPC = "inter-process communication," just two parts of the app talking. | The **wiring** between the UI layer and the Rust brain. Simple, debuggable, and the same format AI tools (MCP) already speak. |
| **Inference backends** | The actual AI that decides the next move. Swappable: a scripted one (free, for demos), the local **Claude CLI**, **Ollama** (runs models on your own machine), or the **Anthropic API**. | The **decision-maker**, plugged in behind one common interface. You can run fully offline/free, or use a frontier model — the rest of the system doesn't change. |
| **Ed25519 signatures** | A standard, lightweight way to cryptographically "sign" data so nobody can forge or alter it. | The **audit trail**. Every action the agent takes gets a signed receipt. You can prove later exactly what it did, and that the log wasn't tampered with. |
| **SQLite (via rusqlite)** | A tiny database that lives in a single file — no server needed. | The agent's **memory**. It remembers every action across sessions, so it knows what it did in past runs. |
| **YAML policy file** | A simple human-readable config file. | The **rulebook**: which actions are allowed, which need a human's approval (e.g. deletes), and the action-per-minute speed limit. |

---

## 5. What's built so far (the safety/governance story)

This isn't just "AI clicks buttons." The hard, valuable parts are the **guardrails**:

- **Permissions** — deny-by-default rulebook. The agent can only do what the policy allows.
- **Human approval** — risky actions (like deleting) **pause** and pop up an Approve/Deny box.
  The agent literally cannot delete without a human's OK.
- **Rate limits** — if an agent goes haywire and loops, it gets cut off (e.g. 60 actions/min).
- **Signed audit trail** — every action is cryptographically signed; we built a separate tool
  that can verify the whole log offline and prove it's authentic.
- **Persistent memory** — it remembers across runs, and feeds that memory back to the AI.

That's the part that's genuinely **hard to copy** and the part enterprises would pay for.

---

## 6. Why it matters (the pitch in 20 seconds)

> "By 2026–2027, AI agents are becoming a real type of user, and MCP made it one-click to point
> one at your apps. The transport got standardized; the *enforcement* didn't. We ship the
> on-device control plane: one Mac app that drops a governance checkpoint in front of any MCP
> agent — every call checked against deny-by-default policy, paused for human approval when it
> matters, and written to a signed, tamper-evident receipt, in zero integration code. Whoever
> owns that checkpoint sits between every agent and every app. The monitor-and-verify tier is
> free so it spreads; a license unlocks the on-device compliance tier. No SaaS, no cloud."

---

## 7. Likely questions & simple answers

**"Isn't this just letting AI use my app? What's new?"**
The *how*. Everyone else uses screenshots + vision (slow, brittle). We give the agent a clean,
typed menu of commands and structured data — so it's fast, reliable, and doesn't break on a
redesign. Plus the safety layer (permissions/approval/audit) is built in, not bolted on.

**"Why a Mac app and not a website?"**
Because the whole point is that nothing leaves your machine. The agents and the apps they drive
run locally; the governance checkpoint and the signed receipts have to run there too, or you're
back to trusting a cloud. It's a Mac app you download — it installs the gateway, walks you
through the macOS permissions, and tails the receipts on-device. macOS-first.

**"Why Rust? Isn't that overkill?"**
The core does security-critical work: checking permissions and signing the audit trail. Rust is
fast and memory-safe, so that core is trustworthy and won't crash. The UI is still normal
React/TypeScript.

**"Does it need the internet / an OpenAI bill?"**
No. It can run a model **locally** (Ollama) or even use the Claude CLI you already have. Cloud
models are an option, not a requirement. Local-first = private and free.

**"What if the AI does something destructive?"**
It can't, by design. Dangerous actions are denied or require human approval, there's a speed
limit, and everything is logged with tamper-proof signatures.

**"Is this real or a slide deck?"**
Real and running. There's a working desktop app where an agent organizes notes by itself, asks
for approval before deleting, and writes a verifiable audit log. It's on GitHub.

**"How do you make money?"**
Freemium, all in one downloaded app. Free forever: the live governance monitor, offline receipt
verification, and the guided setup — that's how it spreads. A license unlocks the compliance
tier: auditor-ready evidence export and cross-app correlation, still entirely on-device. There's
no cloud tier and no SaaS — nothing leaves your machine.

---

## 8. Words to avoid / translate

| Don't say | Say instead |
|---|---|
| "MCP-compatible tool schemas" | "a machine-readable menu of what the app can do" |
| "JSON-RPC over IPC" | "the UI and the core talk to each other in plain messages" |
| "Ed25519-signed receipts" | "tamper-proof receipts for every action" |
| "inference backend" | "the AI that decides the next move (swappable)" |
| "deny-by-default policy" | "a rulebook; nothing's allowed unless you say so" |
| "episodic memory" | "it remembers what it did before" |

---

*One-sentence version to memorize:* **"It's the on-device control plane for AI agents on your
Mac — one app you download that drops a governance checkpoint in front of any MCP agent and shows
you live, signed proof of everything it does, with nothing leaving your machine."**
