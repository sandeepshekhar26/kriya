# kriya CAD Studio — the GUI you watch the agent drive

A kriya-native CAD app with a **live 3D viewport**: an agent edits a real
[Replicad](https://replicad.xyz) (OpenCascade) parametric model through governed actions, and you
**watch the part change on screen** — resize and drill flow, but **deleting a body pauses for
approval**, and every action lands in an audit trail.

This is the **visual counterpart** to [`../replicad-bolt-on`](../replicad-bolt-on):

| | `replicad-bolt-on` | `replicad-studio` (this) |
|---|---|---|
| Agent | **real Claude Code** over MCP (`kriya-mcp`) | a scripted in-app agent (the "Run agent demo" button) |
| Approval | **native macOS dialog** (`--approval gui`) | an in-app approval modal |
| Audit | **Ed25519-signed** receipts (`verify-receipts`) | an in-app audit list |
| What you see | a terminal + the approval popup | **the 3D part changing live** |

Same parametric model, same policy posture (`agent-policy.yaml`) — one proves the cryptographic
governance with a real frontier agent; this one makes it *visual*.

## Run it

```bash
npm install
npm run dev      # http://localhost:5173
```

Hit **▶ Run agent demo** and watch: the agent measures the plate, widens it to 110 mm, thickens it,
drills a centre hole (each ✅ allowed, audited) — then tries to **delete the body** and kriya
**pauses** with *“kriya — approval required”*. Approve and the solid vanishes; Deny and it's blocked.
You can also edit the parameters and fire the actions yourself.

## What it proves

- **Real geometry, in the browser** — Replicad/OpenCascade (WASM) builds the actual B-rep solid,
  boolean-cuts the holes, and reports exact volume + bounding box. No fakes.
- **Same handler, two callers** — the parameter inputs (human) and the agent demo (agent) run the
  *same* governed actions; reads + routine edits flow, destructive ops gate on approval.
- **The pattern, visible** — this is the build-time-adoption story: a CAD app that ships
  agent-operable *and* governed from day one.

## Build for signed receipts

This studio governs in-app (policy → approval → audit list). To add **cryptographically-signed,
tamper-evident** receipts and a **real external agent**, run the actions through the Rust host —
exactly what [`../replicad-bolt-on`](../replicad-bolt-on) does over `kriya-mcp`. The two demos
together are the whole story: provable governance + a GUI you can watch.

## Layout

```
src/replicadKernel.ts   Replicad-in-browser → mesh + volume + bbox (the real CAD kernel)
src/Viewport.tsx        three.js 3D viewport
src/cadModel.ts         the parametric model
src/governance.ts       policy (decide) + the action handlers — mirrors agent-policy.yaml
src/App.tsx             the studio: live render, governed dispatch, approval modal, audit, agent demo
```
