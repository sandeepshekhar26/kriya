# Publishing Runbook

> The planner runs these commands (they're irreversible and need credentials — decision
> [D-004](DECISIONS.md)). The agent prepares; the planner executes. Roadmap item **R2**.
>
> **Status: the initial publish is DONE (2026-06-15).** All four npm packages and the crate are
> live (see "Version state" at the bottom). The names are **unscoped** (`kriya-core`, not
> `@kriya/core`) — the `@kriya` scope step below is historical. This doc is now the **republish**
> runbook: sections 1–5 are the original first-publish steps (kept for reference); the
> **"Republishing for P0.5"** section at the end has the exact, current commands the planner needs.

## Order matters

`kriya-core` → `kriya-inspector` (peer-deps on core) → `create-kriya-app`
(independent). `kriya` → crates.io can go in parallel, but the scaffolder template
swap (last step) depends on it being live.

## 0. One-time setup

```bash
cd /Volumes/WORKSSD/software_for_agents/experiment1

# Note: packages ended up UNSCOPED (kriya-core, kriya-sidecar, …) — no @kriya scope needed.
npm login                                            # browser flow

# crates.io token: https://crates.io/me
cargo login                                          # paste token
```

## 1. kriya-core → npm

```bash
npm run build --workspace kriya-core
npm run test  --workspace kriya-core         # expect 50 passing
( cd packages/core && npm pack --dry-run )           # inspect tarball, NO upload
( cd packages/core && npm publish )                  # publishConfig.access already = public
```

## 2. kriya-inspector → npm

Peer-deps on `kriya-core` — needs step 1 live on npm first.

```bash
npm run build --workspace kriya-inspector
( cd packages/inspector && npm pack --dry-run )
( cd packages/inspector && npm publish )
```

## 3. create-kriya-app → npm

Unscoped. After this, `npm create kriya-app@latest my-app` works for anyone.

```bash
( cd packages/create-kriya-app && npm pack --dry-run )
( cd packages/create-kriya-app && npm publish )
```

## 4. kriya → crates.io (parallel track)

```bash
( cd crates/kriya && cargo publish --dry-run )
( cd crates/kriya && cargo publish )
```

## 5. Swap the scaffolder template to the published crate, then republish

Until this is done, scaffolded apps build only inside the monorepo (the template path-deps the
in-repo crate). **Critical for external users.**

```text
# packages/create-kriya-app/template/src-tauri/Cargo.toml
#   replace: kriya = { path = "../../../crates/kriya" }
#   with:    kriya = "0.1"

# Delete the embedded host source the template no longer needs:
#   template/src-tauri/src/{audit,budget,memory,permissions,protocol}.rs
#   template/src-tauri/src/agent/*.rs
# Keep only: template/src-tauri/src/{main.rs, lib.rs, deterministic.rs}

# Then bump create-kriya-app to 0.2.0 and republish (step 3 again).
```

After the swap, **re-run the fresh-machine smoke** (LAUNCH.md pre-launch checklist) to confirm
a generated app builds against the *published* crate, not the local path.

## Expected failure modes

- `404` on the `@kriya` scope → create the scope first (step 0).
- `403` on first publish → scope/package must be public; `publishConfig.access: public` is
  already set in both package.json files, so this usually means you're not logged in or the
  scope isn't yours.
- `npm create kriya-app@latest` resolves to nothing for ~5–10 min after publish → npm registry
  propagation lag, not a bug.
- `cargo publish` rejects a keyword > 20 chars → ours are fine, but check if you add new ones.

## Version state

**Live on the registries (published 2026-06-15):**

- `kriya-core` — 0.0.1 (npm)
- `kriya-sidecar` — 0.0.1 (npm)
- `kriya-inspector` — 0.3.0 (npm)
- `create-kriya-app` — 0.2.0 (npm; crate-based template)
- `kriya` (crate) — 0.1.0 → **0.1.2** (2026-06-23, feature flags + `memory_recent`) → **0.1.3**
  (published 2026-06-24 per crates.io — the Rust authoring SDK; this doc previously listed it as
  staged, reconciled 2026-07-02) → **0.1.4** (published **2026-07-02** — adds `kriya-hook`, the
  Claude Code hooks adapter, R30). Post-publish verified from the registry:
  `cargo install kriya --version 0.1.4 --bin kriya-hook --no-default-features` installs clean and
  its receipts re-verify with the released `kriya-audit` (exit 0).

**Staged in-repo, not yet published:**

- `kriya-core` — 0.0.2 · `kriya-sidecar` — 0.0.2 (the P0.5 npm republish — see below)

## Republishing for P0.5 (current pending action)

P0.5 (R14–R16) added `recentMemory()` + the `Episode` type to `kriya-core` and `kriya-sidecar`,
and a `memory_recent` handler to the `kriya` crate. npm/crates.io versions are immutable, so the
versions are already bumped in-repo (core/sidecar → 0.0.2, crate → 0.1.1; both apps' `Cargo.lock`
regenerated). **Only external installers need this** — everything in this monorepo uses the local
source. Build/tests are already green; the planner just runs the publishes:

```bash
cd /Volumes/WORKSSD/software_for_agents/experiment1
npm login            # if not already
cargo login          # if not already (only needed for the crate)

# npm — core first (sidecar depends on it):
npm run build --workspace kriya-core    && ( cd packages/core    && npm publish )   # 0.0.2
npm run build --workspace kriya-sidecar && ( cd packages/sidecar && npm publish )   # 0.0.2

# crate (optional — only if external crates.io users should get the memory_recent handler):
( cd crates/kriya && cargo publish --dry-run && cargo publish )                      # 0.1.1
```

Not required for P0.5: `kriya-inspector` and `create-kriya-app` are unchanged by P0.5 — no
republish. (If you later want freshly-scaffolded apps to pull `kriya-core` 0.0.2, bump the
template's `kriya-core` range in `packages/create-kriya-app/template/package.json` and republish
`create-kriya-app` — a separate, optional follow-up.)

## 0.1.2 — optional feature flags (DONE, published 2026-06-23)

`kriya` 0.1.2 made `tauri` and `ureq` **optional, default-on** features
(`default = ["tauri-host", "http-inference"]`) so a true **in-process** embedder can link the
crate with `default-features = false` and pull neither a Tauri runtime nor an HTTP client. It also
folded in the staged P0.5 `memory_recent` handler. Backward-compatible (default unchanged). See
[D-014](DECISIONS.md).

## 0.1.3 — Rust authoring SDK (current pending publish, then the Spent MR)

`kriya` 0.1.3 adds the `registry` module — `Registry`/`wrap_action`/`Action`/`Param`/`Params`/
`json_result` — the Rust counterpart to kriya-core's `wrapAction` (declare an action once → it
generates the MCP tool schema *and* dispatches). See [D-014](DECISIONS.md). Lives in the lean core
(no `tauri`/`ureq`), backward-compatible, 95 crate + 4 registry tests, fmt/clippy clean.

The **Spent** bolt-on depends on `kriya = { version = "0.1.3", default-features = false }`, so
**0.1.3 must be live before the Spent MR goes up.** Commit the crate change, then (planner — [D-004]):

```bash
cd /Volumes/WORKSSD/software_for_agents/experiment1
cargo login                                            # if not already
git add crates/kriya/Cargo.toml crates/kriya/src/lib.rs crates/kriya/src/registry.rs crates/kriya/src/protocol.rs
git commit -m "feat(kriya): Rust authoring SDK (Registry/wrap_action) (0.1.3)"
( cd crates/kriya && cargo publish --dry-run && cargo publish )   # 0.1.3 — irreversible
```

Then the Spent MR: build with network (`cargo build` regenerates Spent's `Cargo.lock` against the
published 0.1.3), then commit + push the PR branch (see the integration target's notes).
