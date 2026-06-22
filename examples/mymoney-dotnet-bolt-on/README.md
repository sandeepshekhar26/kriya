# kriya × MyMoney.Net — the .NET bolt-on (R18 flagship)

The **.NET parallel of the [Actual Budget bolt-on](../actual-budget-bolt-on/)**: give an AI agent
**governed** access to a real, local-first finance app — [MyMoney.Net](https://github.com/MoneyTools/MyMoney.Net)
(a WPF C# personal-finance app, local SQLite, no cloud API for the money model) — by wrapping the
methods it **already has**, in a few lines, no rewrite. The agent can categorize and reconcile, but
**cannot delete a transaction or move money without on-device human approval**, and every action is
signed + audited. Money is the most visceral governance story there is.

> **Why MyMoney.Net.** The 2026‑06‑22 target research (with adversarial verification) found that the
> cleanest cross‑platform pick (Mnemo) had just gone *MCP‑native* — so it's no longer the wedge.
> MyMoney.Net is the strongest remaining narrative target: local SQLite, no cloud API for accounts
> or transactions, and **WPF — the bullseye of the .NET ICP** (regulated Windows desktop). The one
> catch is exactly that: WPF is **Windows‑only**, so this bolt‑on builds and runs on Windows.

## Status (honest)

- The **[`Kriya` .NET binding](../../bindings/dotnet/)** this builds on is shipped + verified
  cross‑platform (25 tests incl. a real‑`kriya-host` integration run; runnable on macOS).
- This **bolt‑on file** is drop‑in integration code written against MyMoney.Net's documented
  `Walkabout.Data` API. It is **built and recorded on Windows** (WPF doesn't build on macOS), and the
  exact method signatures should be confirmed against your MyMoney.Net checkout — they're marked `⚠️ API`
  below. It is deliberately *source‑only* (no `.csproj`): you add it to the MyMoney solution.

## Integrate (Windows)

1. Clone MyMoney.Net and open it in Visual Studio.
2. Add the kriya binding: `dotnet add package Kriya` (or reference `bindings/dotnet/Kriya/Kriya.csproj`).
3. Build the host once: `cargo build -p kriya --bin kriya-host --locked` (from `apps/note-app/src-tauri`),
   or grab a released `kriya-host`.
4. Drop in [`KriyaBoltOn.cs`](KriyaBoltOn.cs), call `KriyaBoltOn.BuildRegistry(myMoney)` at startup
   with your live `MyMoney` instance, and host an agent:

   ```csharp
   var reg = KriyaBoltOn.BuildRegistry(myMoney);
   using var host = Host.Spawn(@"C:\path\to\kriya-host.exe",
       new[] { "--policy", "agent-policy.yaml" },
       new Dictionary<string,string> { ["AGENT_BACKEND"] = "claude-cli" });
   var done = await Host.RunTask(host, reg, "reconcile June's transactions",
       state: new JsonObject(),
       approve: req => /* show a WPF modal, return the human's Yes/No */ ConfirmInUi(req));
   ```

5. The agent categorizes/reconciles freely; the moment it proposes `delete_transaction`, `transfer`,
   or `close_account`, the run **pauses for your approval** (a WPF modal), then signs the receipt.

The policy that gates this is [`agent-policy.yaml`](agent-policy.yaml) — the same shape as the Actual
Budget demo: reads + categorize allowed, destructive/money‑moving require approval, deny‑by‑default,
budget‑capped.

## The one-glance pitch

MyMoney.Net already exposes a typed action surface to *its own code*. kriya makes that surface safe
for an **agent**: permission → approval → budget → a signed, on‑device audit trail — with zero rewrite,
on the desktop where the data and the human are, where no cloud MCP gateway can reach.
