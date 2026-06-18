# kriya × Replicad — governed agent access to a CAD model

The second flagship demo (a sibling to [`../actual-budget-bolt-on`](../actual-budget-bolt-on)).
It bolts **governed, agent-callable actions onto a real [Replicad](https://replicad.xyz)
(OpenCascade) parametric CAD model** — no rewrite, no new API. An external AI agent can resize the
part and drill holes all day, but **deleting a body or resetting the model needs on-device human
approval**, anything unlisted is refused, and **every change is signed** into a tamper-evident
audit log.

> Desktop CAD is the textbook kriya target: the capability lives **only inside the running app**
> (no cloud API), the data is **proprietary design IP**, and a bad edit is **high-stakes**
> (scrapped parts, change-control violations). One before/after demo = the whole pitch, for a
> second vertical beyond finance.

## Run it

```bash
./demo.sh                 # builds, then drives the real kriya-mcp governor like an MCP client
DEMO_SPEED=0 ./demo.sh    # fast, no pauses
CAD_FAKE=1   ./demo.sh    # skip the WASM kernel (analytic geometry) for a quicker run
```

No API key, no install beyond `npm` — the OpenCascade kernel ships as a bundled WASM. You'll see
four acts: the agent revises the part (allowed, signed), tries to destroy geometry (**blocked**),
proceeds once **you** approve, and finally every executed action verified offline.

## Drive it live with Claude Code (native GUI approval)

The same governance, driven by a *real* agent instead of the scripted `demo.py`. **Claude Code**
(the CLI) reads a project-local [`.mcp.json`](.mcp.json.example) and shows the agent's tool calls
inline; `--approval gui` pops a **native macOS dialog** for guarded actions — it works under Claude
Code's TUI (a `tty` prompt would deadlock there). No API key, no server — the kernel is bundled, so
this is lower-friction than the Actual Budget live demo.

```bash
npm install && npm run build
node dist/handler.js --dump > tools.json
( cd ../../crates/kriya && cargo build --release --bin kriya-mcp )   # the governor
cp .mcp.json.example .mcp.json    # then replace /abs/path/to/repo with your absolute repo path
claude                            # approve the "replicad-cad" server; /mcp shows 8 tools
```

Then prompt Claude and watch the gates fire:
- *"Measure the plate, make it 100 mm wide, and add a 12 mm hole at the centre."* → runs freely (✅ each signed, no prompt).
- *"Now delete the body."* → kriya **pauses** and a native dialog pops — **kriya — approval required** — with the action + params and Deny / Approve. **That's the on-camera moment.** Approve → it runs; Deny → hard block. Either way it's signed and audited.
- Cut to proof: `verify-receipts "$TMPDIR/kriya-audit.jsonl"`.

> Restart Claude Code after editing `tools.json` / `agent-policy.yaml` / `--approval` (kriya-mcp reads
> them once at startup). Use `gui` under Claude Code, not `tty`; `deny` for a hard-block shot, never
> `auto` for a live demo.

**Seeing the part change on screen:** unlike the Actual demo (which has its own web UI to watch),
Replicad is a library with no standing window. `export_stl` writes a real STL you can open in any
viewer — or ask for the optional **live 3D viewer** that re-renders the part as Claude edits it, for
full visual parity.

## What it proves

| The agent does… | kriya's verdict |
|---|---|
| `measure`, `get_model`, `export_stl` | ✅ allowed (read) |
| `set_parameter` (resize), `add_hole` (drill) | ✅ allowed (routine edit) |
| `delete_feature`, `delete_body`, `reset_model` | ⏸ **held for human approval** |
| `run_macro` / anything unlisted | ⛔ **refused** (deny-by-default) |

Every allowed action gets an **Ed25519-signed receipt**; `verify-receipts` confirms them offline.
The policy that decides all of this is one readable file: [`agent-policy.yaml`](agent-policy.yaml).

## The bolt-on

The entire integration is [`src/actions.ts`](src/actions.ts) — ~45 lines of `wrapAction(...)` over
the CAD app's existing in-process functions ([`src/cad.ts`](src/cad.ts)). Nothing about agents or
governance leaks into the app; the host (`kriya-mcp`) enforces policy → approval → budget → signed
audit, and the persistent handler ([`src/handler.ts`](src/handler.ts)) just runs the cleared action.

## Real geometry

Geometry is **real Replicad / OpenCascade** by default: the demo builds an actual B-rep solid,
drills holes with boolean cuts, reports exact volume + bounding box, and `export_stl` writes a real
multi-hundred-KB STL you can open in any slicer. `CAD_FAKE=1` swaps in an analytic kernel (closed-form
volume, no WASM) so it also runs in CI / anywhere — the governance behaviour is identical either way.

## See it in the console

Every receipt this demo writes (`$TMPDIR/kriya-audit.jsonl`) loads straight into **kriya Console**'s
Audit view (verified locally), and the actions it uses show up as govern-able tools in the Policy
plane. Finance and CAD, one governance surface.
