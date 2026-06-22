// kriya × MyMoney.Net — drop-in bolt-on (R18 flagship). The .NET parallel of the Actual Budget JS
// bolt-on: wrap the methods MyMoney.Net ALREADY has so an AI agent can drive them through
// permission -> human approval -> budget -> signed audit, on-device. No rewrite.
//
// This is *integration code* you add to the MyMoney.Net solution on Windows (WPF is Windows-only).
// It references the `Kriya` NuGet package and MyMoney's `Walkabout.Data` model. The method names
// below mirror MyMoney.Net's documented data API; lines marked "⚠️ API" are the spots to confirm
// against your checkout (exact signatures vary by version) — the kriya wrapAction *pattern* is the
// point and is stable.

using System.Text.Json.Nodes;
using Kriya;
using Walkabout.Data; // MyMoney.Net's data model: the MyMoney container, Account, Transaction

namespace MyMoney.KriyaBoltOn;

public static class KriyaBoltOn
{
    /// <summary>Build a governed action registry over a live MyMoney instance. Pass the result to
    /// <c>Host.RunTask(host, registry, ...)</c>. Reads + categorize run freely; delete / transfer /
    /// close pause for a human (per agent-policy.yaml).</summary>
    public static Registry BuildRegistry(Walkabout.Data.MyMoney money)
    {
        var reg = new Registry();

        // ── reads + low-stakes edits: allowed (budget-metered, each signed) ──

        reg.RegisterAction("list_accounts", "List accounts with their balances.",
            (_, _) =>
            {
                var arr = new JsonArray();
                foreach (var a in money.Accounts.GetAccounts()) // ⚠️ API: account enumeration
                    arr.Add(new JsonObject { ["id"] = a.Id, ["name"] = a.Name, ["balance"] = (double)a.Balance });
                return ActionResult.Ok(arr);
            });

        reg.RegisterAction("list_transactions", "List recent transactions for an account.",
            (p, _) =>
            {
                var acct = money.Accounts.FindAccount(p["account"]!.GetValue<string>()); // ⚠️ API
                var txns = money.Transactions.GetTransactionsFrom(acct);                  // ⚠️ API
                var arr = new JsonArray();
                foreach (var t in txns)
                    arr.Add(new JsonObject { ["id"] = t.Id, ["payee"] = t.PayeeName, ["amount"] = (double)t.Amount, ["category"] = t.CategoryName });
                return ActionResult.Ok(arr);
            },
            parameters: new() { ["account"] = P.Required(P.Str) });

        reg.WrapAction(
            args =>
            {
                var t = money.Transactions.FindTransactionById((long)args[0]!); // ⚠️ API
                t.Category = money.Categories.GetOrCreateCategory((string)args[1]!, /* type */ null); // ⚠️ API
                return t.Id;
            },
            id: "categorize_transaction", description: "Assign a category to a transaction.",
            parameters: new() { ["id"] = P.Required(P.Num), ["category"] = P.Required(P.Str) },
            mapParams: p => new object?[] { p["id"]!.GetValue<long>(), p["category"]!.GetValue<string>() });

        // ── destructive / money-moving: policy = require_approval (pauses for a human) ──

        reg.WrapAction(
            args =>
            {
                var t = money.Transactions.FindTransactionById((long)args[0]!); // ⚠️ API
                money.RemoveTransaction(t);                                     // ⚠️ API
                return args[0];
            },
            id: "delete_transaction", description: "Permanently delete a transaction.",
            parameters: new() { ["id"] = P.Required(P.Num) },
            mapParams: p => new object?[] { p["id"]!.GetValue<long>() });

        reg.WrapAction(
            args =>
            {
                var from = money.Accounts.FindAccount((string)args[1]!); // ⚠️ API
                var to = money.Accounts.FindAccount((string)args[2]!);
                return money.Transfer((decimal)args[0]!, from, to);      // ⚠️ API: Transfer(amount, from, to)
            },
            id: "transfer", description: "Transfer money between two accounts.",
            parameters: new() { ["amount"] = P.Required(P.Num), ["from"] = P.Required(P.Str), ["to"] = P.Required(P.Str) },
            mapParams: p => new object?[] { p["amount"]!.GetValue<decimal>(), p["from"]!.GetValue<string>(), p["to"]!.GetValue<string>() });

        reg.WrapAction(
            args =>
            {
                var acct = money.Accounts.FindAccount((string)args[0]!); // ⚠️ API
                acct.IsClosed = true;                                    // ⚠️ API: close = set flag, or money.RemoveAccount(acct)
                return args[0];
            },
            id: "close_account", description: "Close an account.",
            parameters: new() { ["account"] = P.Required(P.Str) },
            mapParams: p => new object?[] { p["account"]!.GetValue<string>() });

        return reg;
    }
}
