# Actor identity per action (R8)

Every action a kriya agent takes is recorded in an **Ed25519-signed receipt**. R8 adds
*who took it* — an `actor` carrying the **agent** that drove the action and the **user**
(operator) it acted for — **inside the signed bytes**. Attribution is therefore
tamper-evident: rewriting who-did-what invalidates the signature, exactly like rewriting
the params would.

```jsonc
{
  "step_id": "…", "action_id": "delete_transaction", "params": { "id": "txn-2" },
  "success": true, "ts_ms": 1781808031430,
  "actor": { "agent": "claude-desktop", "user": "alice" },   // ← R8
  "public_key": "…", "signature": "…"
}
```

The field is **optional and skipped when absent**, so receipts written before R8 (and any
caller that supplies no identity) sign byte-identically to the original format — every
existing verifier keeps validating unchanged.

## Run the demo

```bash
./demo.sh
```

It drives the real `kriya-mcp` binary as an external agent (`claude-desktop`, acting for
`alice`) through two governed actions, then:

1. **verifies the audit log offline** with `verify-receipts` — both receipts pass;
2. **reads back the attribution** from each signed receipt;
3. **tampers** with the operator on one receipt and re-verifies — the forged attribution is
   **rejected**, proving who-did-what is signed, not editable after the fact.

## Where it comes from

- The host stamps the actor in [`crates/kriya/src/audit.rs`](../../crates/kriya/src/audit.rs)
  (the `Actor` type + `Receipt.actor`).
- The in-process host (`agent::host`) resolves it from the run request (`agent_id` / `user_id`),
  falling back to the backend name + OS user.
- The MCP server stamps it from the binary's `--actor` / `--user` flags (used here), so an
  external agent driving over MCP is attributed too.

This is the open-core **primitive**. Richer identity *management* (SSO/OIDC, RBAC, per-user
dashboards) is a separate, paid concern that builds on this signed field.
