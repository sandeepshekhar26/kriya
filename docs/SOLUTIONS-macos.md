# Kriya Gateway on macOS — solutions guide

The Kriya Gateway puts **one governance layer** in front of the apps an AI agent drives on a Mac:
every action runs through **policy → human approval → budget → signed audit → on-device
attestation**, regardless of how the app exposes itself. This guide is practical: how to install it,
what reach-in can and can't do, and how the three fronts fit together.

The governance core is identical across every front. What changes is only *how the gateway reaches
the app*.

---

## What you get

Kriya is the **on-device control plane** for AI agents on your Mac. It's **freemium**: the **free**
tier gives you a live governance monitor, offline receipt verification, and guided setup; a license
unlocks the **compliance tier** — auditor-ready evidence export and cross-app correlation. Everything
runs **on-device** — no SaaS, no accounts, no cloud. **macOS.** Works with any MCP agent (Claude
Desktop, Cursor, …): every call is checked against deny-by-default policy, paused for human approval
when it matters, and written to a signed, tamper-evident receipt — zero integration code.

---

## 1. Install and the one-time Accessibility grant

### Download and install

**Download Kriya Console, drag it to `/Applications`, and open it.** (On disk the bundle is
`Kriya Gateway.app` — that's the app you'll see in the macOS permission dialogs below.) On first
launch the in-app setup walks you through it: it grants the macOS permissions reach-in needs and
wires your MCP client's config for you — no terminal required.

> Advanced / building from source: you can also build the signed `.app` yourself with
> `bash scripts/macos/build-gateway-app.sh` (→ `dist/macos/Kriya Gateway.app + KriyaGateway.dmg`),
> then open the `.dmg` and drag **Kriya Gateway.app** to `/Applications`.

### Grant Accessibility once

reach-in (Front 2) reads an app's macOS accessibility tree, which requires the **Accessibility**
permission. The in-app setup requests this for you on first launch; the GUI is the path most people
should use. If you prefer to do it by hand:

> System Settings → Privacy & Security → Accessibility → add **Kriya Gateway.app** → toggle ON

For headless or scripted setups, a terminal preflight is available as an **option** — it checks the
grant, opens that exact pane for you, lists the apps reach-in can target, and prints a ready-to-paste
Claude Desktop snippet:

```bash
"/Applications/Kriya Gateway.app/Contents/MacOS/kriya-gateway" doctor --app "Numbers"
```

### Why it MUST be the `.app` bundle (the loose-binary trap)

macOS gates Accessibility behind **TCC**, which keys a permission to a stable **app identity**
(the bundle's `CFBundleIdentifier`, here `com.kriya.gateway`) — **not** to a file path.

We hit this live with Claude Desktop:

- A **loose binary** spawned by an Electron host (Claude Desktop) **cannot** be granted
  Accessibility. macOS will let you add it to the list, but the grant never sticks, so the gateway
  stays untrusted and reach-in can't read the tree.
- The **signed `.app`** with a fixed bundle identifier **can** be granted, and the grant persists.

So your MCP client's `command` must point at the binary **inside** the bundle —
`…/Kriya Gateway.app/Contents/MacOS/kriya-gateway` — never at a bare binary. `kriya-gateway doctor`
warns you when it detects it is running loose.

---

## 2. "Will all my apps be listed and governed automatically?"

**No.** There is **no auto-governance** and nothing is governed silently.

- `kriya-gateway doctor` performs reach-in **discovery**: it lists the running, user-facing apps as
  *candidates*. That is a menu, not a policy — listing an app does nothing to it.
- You **opt an app in** explicitly by configuring a gateway server for it (one `mcpServers` entry,
  with `--app "<Name>"`) and giving it a **policy**. Until you do, the gateway never touches it.
- Even an opted-in app is fully gated: every single action still passes through your policy and (in
  `--approval gui` mode) a human approval modal before anything happens.

Discovery answers "what *could* I govern?"; you decide "what *do* I govern?".

---

## 3. How typed, governed actions come out of accessibility (reach-in)

For an app with no MCP server and no API, the gateway synthesizes a governed tool surface from the
accessibility (AX) tree:

1. **Read the AX tree** of the target app (buttons, fields, checkboxes, menus, …).
2. **Synthesize typed tools** from those elements — the same small, typed action vocabulary the
   in-process host uses:
   - `press` — click a button / control
   - `set_value` — set a field/control to a value
   - `type_text` — type into the focused element
   - `press_key` — send a key / shortcut
3. **Route every call through the one governance core** — exactly as the proxy and bolt-on do:
   - **policy** — deny-by-default; reads allowed, destructive/spend actions require approval
   - **approval modal** — `--approval gui` shows a native modal a human approves or denies
   - **budget** — spend/rate limits enforced before the action runs
   - **signed audit** — every decision is an Ed25519-signed receipt in a JSONL log. By default the
     log lands in the **standard on-device location `~/.kriya/audit/`** (a per-front file, e.g.
     `reach-in-numbers.jsonl`) — the directory the **Kriya control-plane app auto-discovers and
     tails**, so you open the app and see governance with no file to import. `--audit-log <path>`
     still overrides for ad-hoc inspection.
   - **attestation** — with a pinned signing key, the log opens with an on-device attestation receipt

The agent never drives pixels or scrapes the screen; it calls **typed tools**, and the gateway turns
a cleared call into the corresponding AX action.

### Honest coverage limits

reach-in's reach is the AX tree's reach, so coverage varies by app:

- **Strong:** native macOS **control / form** apps — buttons, toggles, steppers, text fields,
  menus. This is where reach-in shines (press a button, set a field, run a command).
- **Weak / degraded:** **Electron, Qt, and web UIs** expose thin or non-standard AX trees; many
  controls are invisible or unlabeled.
- **Weak for bulk data entry:** **spreadsheets** (e.g. Numbers) surface a sparse grid over AX —
  good for pressing toolbar controls and toggles, poor for reliable cell-by-cell data entry.

When an app speaks MCP, prefer the **proxy** (Front 1). When it's a kriya-instrumented app, prefer
**serve** (the bolt-on). reach-in is the front for the apps the other two can't reach.

---

## 4. How a "Spent-kind" app (built on `--exec`) is discovered — NOT via accessibility

A kriya-instrumented app like **Spent** is **not** reached through accessibility. It ships its own
governed surface, which is topologically the **strongest** front because the gateway governs the
app's **real, named handlers** — not synthesized approximations of its UI.

How it works:

1. The app declares a **kriya manifest**: the SDK's `getToolSchemas()` is dumped to a `tools.json`
   (the "dump-tools" step) describing its real named actions (e.g. `add_transaction`,
   `delete_container`).
2. The app provides an **`--exec` handler** — a command the gateway runs per cleared action; it
   reads `{"action","params"}` on stdin and performs the real operation in-app.
3. The gateway runs in **`serve`** mode over that manifest + handler:

   ```bash
   kriya-mcp --tools tools.json --policy policy.yaml --exec "node handler.js" --approval gui
   ```

   (`kriya-gateway serve …` is the same bolt-on; point the client at the `kriya-mcp` binary.)

Because the gateway governs the app's **own** named handlers, there's no AX guesswork, no coverage
gap, and the typed tools match the app's real domain. This is the deepest integration — and the path
a new local-first AI app should build toward.

---

## 5. The 4-tier reach model — support every app, govern it (D-017)

The gateway reaches a target by the **richest mechanism the app supports**, and **all four share the
identical** policy → approval → budget → signed audit → attestation core. The more instrumented the
app, the **richer** the governance; computer-use is the **universal floor**, so *no app is unsupported.*

| The app you want to govern | Front | How the gateway reaches it | Governance richness |
|---|---|---|---|
| A **kriya-instrumented** app (manifest + `--exec`, e.g. Spent) | **serve / bolt-on** (`kriya-mcp --tools … --exec …`) | the app's **real named handlers** | **Richest** — deny/approve a *named* domain action (`delete_transaction`) |
| An app that already speaks **MCP** | **proxy** (`kriya-gateway proxy -- <server-cmd>`) | transparent stdio proxy, **zero changes** | Strong — governs the server's declared tools |
| An **uninstrumented** app with an AX tree (e.g. Numbers) | **reach-in** (`kriya-gateway reach-in --app "<Name>"`) | typed tools from the **macOS AX tree** | Medium — gate a named element ("press Delete"); coverage-bounded |
| **Any app at all** (the floor) | **computer-use** (`kriya-gateway computer-use` / `router`) | system-wide **pixels** — CGEvent click/type/scroll + screenshot | **Coarse** — gate/audit *clicks & keystrokes*, not named actions |

Pick the strongest front the app supports (serve → proxy → reach-in → computer-use); the **router**
subcommand will (v2) auto-select per app. **"Support everything"** = computer-use is the universal
floor; **"sell governance"** = the value is richest where the app is instrumented.

**Vs an ungoverned computer-use agent (e.g. Cowork):** reach is the same at the floor, but every kriya
action is **policy-gated + signed + on-device + vendor-neutral** (it governs *any* MCP agent, and the
rules are the **app owner's**, not the AI client's — which can't be toggled off). That governance, not
the ability to drive apps, is the moat.

---

## Quick reference

```bash
# Preflight: Accessibility grant + bundle check + candidate apps + Claude Desktop snippet
"…/Kriya Gateway.app/Contents/MacOS/kriya-gateway" doctor --app "Numbers"

# Front 1 — proxy an existing MCP server (zero changes)
kriya-gateway proxy --approval gui -- node actual-mcp-server.js

# Front 2 — reach into an uninstrumented app via accessibility
kriya-gateway reach-in --app "Numbers" --approval gui

# Front 3 — governed computer-use (ANY app, system-wide): the universal floor (alias: router)
kriya-gateway computer-use --approval gui

# serve / bolt-on — govern a kriya-instrumented app's real handlers
kriya-mcp --tools tools.json --policy policy.yaml --exec "node handler.js" --approval gui
```

For packaging and Developer ID + notarization (real distribution), see
[`scripts/macos/README.md`](../scripts/macos/README.md).
