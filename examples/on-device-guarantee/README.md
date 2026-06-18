# On-device guarantee (R13)

Regulated and privacy-sensitive apps often can't use cloud agents at all — the selling point
is **nothing leaves the device**, and being able to *prove* it. R13 makes that a first-class,
audited posture.

Set `on_device: true` in the agent policy and the **in-process host**:

1. **refuses to run** with an inference backend that egresses to a remote service — before a
   single action (or inference call) happens; and
2. signs an **attestation receipt** (`kriya.attestation.on_device`) recording that the run was
   sealed and which local backend drove it — so the audit log itself attests the guarantee,
   verifiable offline like any other receipt.

Backends declare their reach honestly via `NetworkProfile`:

| Backend | Profile | On-device? |
|---|---|---|
| scripted / deterministic | `no-network` | ✅ |
| Ollama on loopback | `localhost-only` | ✅ |
| Ollama pointed at a remote host | `remote` | ❌ |
| `claude` CLI | `remote` (reaches the cloud) | ❌ |
| Anthropic API | `remote` | ❌ |

A new backend defaults to `remote` — it is never *silently* treated as on-device.

## Run the demo

```bash
./demo.sh
```

It runs the **same sealed policy** two ways against the real `kriya-host` sidecar:

- **A — on-device backend** (deterministic/local): the run proceeds, signs an attestation, and
  the whole audit log verifies offline.
- **B — egressing backend** (`AGENT_BACKEND=anthropic`): the run is refused before it starts and
  nothing is signed. (No API key needed — the refusal precedes any inference call.)

## Where it comes from

- `NetworkProfile` + `Inference::network_profile()` —
  [`crates/kriya/src/agent/inference/mod.rs`](../../crates/kriya/src/agent/inference/mod.rs)
- enforcement + attestation in the host loop —
  [`crates/kriya/src/agent/host.rs`](../../crates/kriya/src/agent/host.rs)
- the `on_device` policy flag —
  [`crates/kriya/src/permissions.rs`](../../crates/kriya/src/permissions.rs)
