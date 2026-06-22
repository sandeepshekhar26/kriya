using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Kriya;

/// <summary>
/// A live connection to a <c>kriya-host</c> sidecar — the Rust agent host. Spawns the binary (or wraps
/// raw streams for tests) and speaks its newline-delimited JSON protocol over stdio. The agent loop,
/// the inference backend, and the whole safety layer (policy, approval, budget, signed audit) run
/// inside that separate process — which your app/UI can't tamper with — while your .NET process just
/// runs the typed actions the host has already cleared. The cross-shell binding of kriya for .NET.
/// </summary>
public sealed class Host : IDisposable
{
    private readonly TextWriter _stdin;
    private readonly Process? _proc;
    private readonly object _writeLock = new();
    private readonly ConcurrentDictionary<string, TaskCompletionSource<List<Episode>>> _memoryWaiters = new();
    private int _memorySeq;
    private bool _closed;

    /// <summary>The host wants this action executed; reply via <see cref="SendActionResult"/>.</summary>
    public event Action<ActionRequest>? OnAction;
    /// <summary>A guarded action needs a human; reply via <see cref="SendApproval"/>.</summary>
    public event Action<ApprovalRequest>? OnApproval;
    /// <summary>Step-mode pause; reply via <see cref="SendStepAdvance"/>.</summary>
    public event Action<AwaitStep>? OnAwaitStep;
    /// <summary>The run finished.</summary>
    public event Action<Done>? OnDone;
    /// <summary>Inspector/telemetry line.</summary>
    public event Action<LogEntry>? OnLog;
    /// <summary>A stdout line that wasn't valid JSON or had an unknown type (carries the raw line).</summary>
    public event Action<string>? OnParseError;
    /// <summary>The host process exited (carries the exit code, or null if killed by signal).</summary>
    public event Action<int?>? OnExit;

    /// <summary>Construct from raw streams (handy for tests). Usually use <see cref="Spawn"/>.</summary>
    public Host(TextWriter stdin, TextReader stdout, Process? proc = null)
    {
        _stdin = stdin;
        _proc = proc;
        var reader = new Thread(() => ReadLoop(stdout)) { IsBackground = true, Name = "kriya-host-reader" };
        reader.Start();
        if (proc is not null)
        {
            proc.EnableRaisingEvents = true;
            proc.Exited += (_, _) =>
            {
                FailPendingMemory("kriya-host exited before replying");
                int? code = proc.HasExited ? proc.ExitCode : null;
                OnExit?.Invoke(code);
            };
        }
    }

    /// <summary>Spawn the <c>kriya-host</c> binary and connect to it. stderr is inherited so the host's
    /// banner and governance log show up in your console.</summary>
    public static Host Spawn(string binaryPath, IEnumerable<string>? args = null, IDictionary<string, string>? env = null)
    {
        var psi = new ProcessStartInfo
        {
            FileName = binaryPath,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = false, // inherit
            UseShellExecute = false,
        };
        if (args is not null) foreach (var a in args) psi.ArgumentList.Add(a);
        if (env is not null) foreach (var kv in env) psi.Environment[kv.Key] = kv.Value;
        var proc = Process.Start(psi) ?? throw new InvalidOperationException("failed to start kriya-host");
        return new Host(proc.StandardInput, proc.StandardOutput, proc);
    }

    private void ReadLoop(TextReader stdout)
    {
        try
        {
            string? line;
            while ((line = stdout.ReadLine()) is not null)
            {
                var trimmed = line.Trim();
                if (trimmed.Length > 0) DispatchLine(trimmed);
            }
        }
        catch { /* stream closed */ }
        FailPendingMemory("kriya-host stdout closed");
    }

    /// <summary>Parse + route one outbound (host→app) line. Internal so tests can drive framing directly.</summary>
    internal void DispatchLine(string line)
    {
        JsonObject? msg;
        try { msg = JsonNode.Parse(line) as JsonObject; }
        catch { OnParseError?.Invoke(line); return; }
        if (msg is null) { OnParseError?.Invoke(line); return; }

        var type = msg["type"]?.GetValue<string>();
        var data = msg["data"] as JsonObject ?? new JsonObject();
        switch (type)
        {
            case "action": OnAction?.Invoke(ActionRequest.FromWire(data)); break;
            case "approval": OnApproval?.Invoke(ApprovalRequest.FromWire(data)); break;
            case "await_step": OnAwaitStep?.Invoke(AwaitStep.FromWire(data)); break;
            case "done": OnDone?.Invoke(Done.FromWire(data)); break;
            case "log": OnLog?.Invoke(LogEntry.FromWire(data)); break;
            case "memory":
                var reqId = data["requestId"]?.GetValue<string>();
                var episodes = new List<Episode>();
                if (data["episodes"] is JsonArray arr)
                    foreach (var e in arr)
                        if (e is JsonObject eo) episodes.Add(Episode.FromWire(eo));
                if (reqId is not null && _memoryWaiters.TryRemove(reqId, out var tcs)) tcs.TrySetResult(episodes);
                else OnParseError?.Invoke(line);
                break;
            default: OnParseError?.Invoke(line); break;
        }
    }

    private void Send(string type, JsonObject data)
    {
        var msg = new JsonObject { ["type"] = type, ["data"] = data };
        var s = msg.ToJsonString();
        lock (_writeLock)
        {
            _stdin.Write(s);
            _stdin.Write('\n');
            _stdin.Flush();
        }
    }

    /// <summary>Begin an autonomous run.</summary>
    public void Start(StartRequest req) => Send("start", req.ToWire());
    /// <summary>Report the result of an action the host asked you to run.</summary>
    public void SendActionResult(ActionResultMsg result) => Send("action_result", result.ToWire());
    /// <summary>Answer a pending approval request.</summary>
    public void SendApproval(ApprovalResponse response) => Send("approval_response", response.ToWire());
    /// <summary>Advance or stop a step-mode run.</summary>
    public void SendStepAdvance(StepAdvance advance) => Send("step_advance", advance.ToWire());

    /// <summary>Read the newest episodes from the host's durable memory — the .NET equivalent of the
    /// Tauri <c>agent_memory_recent</c> command. Newest first; throws on timeout or if the host exits.</summary>
    public async Task<IReadOnlyList<Episode>> RecentMemoryAsync(int? limit = null, TimeSpan? timeout = null)
    {
        var requestId = $"mem-{Interlocked.Increment(ref _memorySeq)}";
        var tcs = new TaskCompletionSource<List<Episode>>(TaskCreationOptions.RunContinuationsAsynchronously);
        _memoryWaiters[requestId] = tcs;

        var data = new JsonObject { ["requestId"] = requestId };
        if (limit is not null) data["limit"] = limit;
        Send("memory_recent", data);

        using var cts = new CancellationTokenSource(timeout ?? TimeSpan.FromSeconds(5));
        await using var reg = cts.Token.Register(() =>
        {
            if (_memoryWaiters.TryRemove(requestId, out var t))
                t.TrySetException(new TimeoutException("RecentMemory: timed out waiting for the host"));
        });
        return await tcs.Task.ConfigureAwait(false);
    }

    private void FailPendingMemory(string reason)
    {
        foreach (var key in _memoryWaiters.Keys.ToList())
            if (_memoryWaiters.TryRemove(key, out var tcs))
                tcs.TrySetException(new IOException(reason));
    }

    /// <summary>Close stdin (signals shutdown) and kill the child if we own it.</summary>
    public void Close()
    {
        if (_closed) return;
        _closed = true;
        FailPendingMemory("Host closed");
        try { _stdin.Close(); } catch { /* already closed */ }
        try { if (_proc is { HasExited: false }) _proc.Kill(); } catch { /* already gone */ }
    }

    public void Dispose() => Close();

    // ── high-level RunTask ───────────────────────────────────────────────────

    /// <summary>Drive a single run to completion: send start, execute each action via
    /// <paramref name="dispatch"/>, answer approvals, gate step-mode, and resolve with the
    /// <see cref="Done"/> summary.</summary>
    public static Task<Done> RunTask(Host host, StartRequest req,
        Func<ActionRequest, ActionResultMsg> dispatch,
        Func<ApprovalRequest, bool>? approve = null,
        Func<AwaitStep, bool>? onStep = null,
        Action<LogEntry>? onLog = null)
    {
        var tcs = new TaskCompletionSource<Done>(TaskCreationOptions.RunContinuationsAsynchronously);
        host.OnAction += r => host.SendActionResult(dispatch(r));
        host.OnApproval += r => host.SendApproval(new ApprovalResponse { StepId = r.StepId, Approved = approve?.Invoke(r) ?? false });
        host.OnAwaitStep += ev => host.SendStepAdvance(new StepAdvance { GateId = ev.GateId, Proceed = onStep?.Invoke(ev) ?? true });
        if (onLog is not null) host.OnLog += onLog;
        host.OnDone += d => tcs.TrySetResult(d);
        host.Start(req);
        return tcs.Task;
    }

    /// <summary>Registry-driven run: tools come from the <paramref name="registry"/> and each action is
    /// dispatched to its handler. <paramref name="state"/> is sent back each step (mutate it in your
    /// handlers, or pass the app's live state object).</summary>
    public static Task<Done> RunTask(Host host, Registry registry, string goal, JsonObject state,
        Func<ApprovalRequest, bool>? approve = null,
        Func<AwaitStep, bool>? onStep = null,
        Action<LogEntry>? onLog = null,
        bool resume = false, bool stepMode = false, string? agentId = null, string? userId = null)
    {
        var req = new StartRequest
        {
            Goal = goal,
            State = state,
            Tools = registry.ToolSchemas(),
            Resume = resume,
            StepMode = stepMode,
            AgentId = agentId,
            UserId = userId,
        };
        return RunTask(host, req,
            dispatch: ar =>
            {
                var result = registry.DispatchAction(ar.ActionId, ar.Params,
                    new ActionContext { Caller = "agent", StepId = ar.StepId });
                return new ActionResultMsg
                {
                    StepId = ar.StepId,
                    Success = result.Success,
                    Data = result.Data,
                    Error = result.Error,
                    State = state,
                };
            },
            approve, onStep, onLog);
    }
}
