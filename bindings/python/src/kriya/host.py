"""Host the kriya agent runtime from Python -- spawn the ``kriya-host`` binary and drive a
governed run over its stdio NDJSON protocol.

This is the Python mirror of ``kriya-sidecar`` (``SidecarHost`` + ``runTask``). The agent loop, the
inference backend, and the whole safety layer (policy, approval, budget, signed audit, memory) run
**inside that separate process** -- which your UI can't tamper with -- while your Python process
only ever runs the typed actions the host has already cleared, the same handlers a button click
would call. So a PyQt/PySide/Tk app, a FreeCAD/Blender plugin, or a data tool gains a governed,
agent-callable surface by registering actions and handing them to :func:`run_task`.

The host pushes events back on a background reader thread; subscribe with :meth:`Host.on` and reply
with the ``send_*`` methods, or use the high-level :func:`run_task` loop.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from typing import Any, Callable, Dict, List, Optional, Union

from .protocol import (
    ActionRequest,
    ActionResultMsg,
    ApprovalRequest,
    ApprovalResponse,
    AwaitStep,
    Done,
    Episode,
    LogEntry,
    StartRequest,
    StepAdvance,
)
from .registry import Registry
from .types import ActionContext

# Event names the host pushes back (snake_case, idiomatic for Python):
#   "action", "approval", "await_step", "done", "log", "parse_error", "exit"
_EVENTS = ("action", "approval", "await_step", "done", "log", "parse_error", "exit")


class Host:
    """A live connection to a ``kriya-host`` sidecar.

    Construct from raw streams (handy for tests) or, more usually, with :meth:`Host.spawn`.
    Listeners run on the background reader thread; keep them quick and thread-aware.
    """

    def __init__(
        self,
        stdin: Any,
        stdout: Optional[Any] = None,
        proc: Optional[subprocess.Popen] = None,
    ) -> None:
        self._stdin = stdin
        self._stdout = stdout
        self._proc = proc
        self._listeners: Dict[str, List[Callable[..., None]]] = {
            ev: [] for ev in _EVENTS
        }
        # Pending memory_recent queries keyed by the requestId we minted, so concurrent calls
        # don't cross replies. Guarded by _lock; resolved when the matching `memory` line arrives.
        self._memory_waiters: Dict[str, Callable[[List[Episode], Optional[str]], None]] = {}
        self._memory_seq = 0
        self._lock = threading.Lock()
        self._reader: Optional[threading.Thread] = None

    # -- construction -------------------------------------------------------

    @classmethod
    def spawn(
        cls,
        binary_path: str,
        args: Optional[List[str]] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> "Host":
        """Spawn the ``kriya-host`` binary and connect to it.

        ``args`` are extra CLI flags, e.g. ``["--policy", "policy.yaml", "--script", "demo.json"]``.
        stderr is inherited so the host's banner and governance log show up in your console.
        """
        proc = subprocess.Popen(
            [binary_path, *(args or [])],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,  # inherit -> host banner + governance log visible
            text=True,
            bufsize=1,  # line-buffered: dispatch each NDJSON line as it arrives
            env=env if env is not None else os.environ.copy(),
        )
        if proc.stdin is None or proc.stdout is None:
            raise RuntimeError("kriya-host did not expose stdio pipes")
        host = cls(stdin=proc.stdin, stdout=proc.stdout, proc=proc)
        host._start_reader()
        return host

    def _start_reader(self) -> None:
        self._reader = threading.Thread(
            target=self._read_loop, name="kriya-host-reader", daemon=True
        )
        self._reader.start()

    def _read_loop(self) -> None:
        try:
            assert self._stdout is not None
            for line in self._stdout:  # text mode, one NDJSON object per line
                stripped = line.strip()
                if stripped:
                    self._dispatch_line(stripped)
        finally:
            # The host is gone -- no `memory` line is coming, so fail in-flight queries rather
            # than letting them time out, then announce the exit.
            self._fail_pending_memory("kriya-host exited before replying")
            code = self._proc.poll() if self._proc else None
            self._emit("exit", code)

    # -- subscription -------------------------------------------------------

    def on(self, event: str, listener: Callable[..., None]) -> "Host":
        if event not in self._listeners:
            raise ValueError(f"unknown event {event!r}; expected one of {_EVENTS}")
        self._listeners[event].append(listener)
        return self

    def off(self, event: str, listener: Callable[..., None]) -> "Host":
        if event in self._listeners and listener in self._listeners[event]:
            self._listeners[event].remove(listener)
        return self

    # -- inbound (app -> host) ---------------------------------------------

    def start(self, request: StartRequest) -> None:
        """Begin an autonomous run."""
        self._send("start", request.to_wire())

    def send_action_result(self, result: ActionResultMsg) -> None:
        """Report the result of an action the host asked you to run."""
        self._send("action_result", result.to_wire())

    def send_approval(self, response: ApprovalResponse) -> None:
        """Answer a pending approval request."""
        self._send("approval_response", response.to_wire())

    def send_step_advance(self, advance: StepAdvance) -> None:
        """Advance or stop a step-mode run."""
        self._send("step_advance", advance.to_wire())

    def recent_memory(
        self, limit: Optional[int] = None, timeout: float = 5.0
    ) -> List[Episode]:
        """Read the newest episodes from the host's durable memory store (newest first).

        The sidecar equivalent of the Tauri ``agent_memory_recent`` command, so a Python app can
        power an inspector/memory view from the same governed host. Raises :class:`TimeoutError`
        if the host doesn't answer within ``timeout`` seconds, or :class:`RuntimeError` if the host
        reports an error or exits first.
        """
        with self._lock:
            self._memory_seq += 1
            request_id = f"mem-{self._memory_seq}"

        box: Dict[str, Any] = {}
        done = threading.Event()

        def waiter(episodes: List[Episode], error: Optional[str]) -> None:
            box["episodes"] = episodes
            box["error"] = error
            done.set()

        with self._lock:
            self._memory_waiters[request_id] = waiter

        data: Dict[str, Any] = {"requestId": request_id}
        if limit is not None:
            data["limit"] = limit
        self._send("memory_recent", data)

        if not done.wait(timeout):
            with self._lock:
                self._memory_waiters.pop(request_id, None)
            raise TimeoutError("recent_memory: timed out waiting for the host")
        if box.get("error"):
            raise RuntimeError(box["error"])
        return box.get("episodes", [])

    def close(self) -> None:
        """Close stdin (signals shutdown) and terminate the child if we own it."""
        self._fail_pending_memory("Host closed")
        try:
            self._stdin.close()
        except Exception:
            pass
        if self._proc is not None:
            try:
                self._proc.terminate()
            except Exception:
                pass

    # -- internals ----------------------------------------------------------

    def _send(self, type_: str, data: Any) -> None:
        self._stdin.write(json.dumps({"type": type_, "data": data}) + "\n")
        self._stdin.flush()

    def _fail_pending_memory(self, reason: str) -> None:
        with self._lock:
            waiters = list(self._memory_waiters.values())
            self._memory_waiters.clear()
        for waiter in waiters:
            waiter([], reason)

    def _dispatch_line(self, line: str) -> None:
        """Parse one NDJSON line and emit the matching event. Synchronous and side-effect-only,
        so tests can drive it directly without a real subprocess."""
        try:
            message = json.loads(line)
        except (json.JSONDecodeError, TypeError):
            self._emit("parse_error", line)
            return
        msg_type = message.get("type") if isinstance(message, dict) else None
        data = message.get("data") if isinstance(message, dict) else None
        data = data if isinstance(data, dict) else {}

        if msg_type == "action":
            self._emit("action", ActionRequest.from_wire(data))
        elif msg_type == "approval":
            self._emit("approval", ApprovalRequest.from_wire(data))
        elif msg_type == "await_step":
            self._emit("await_step", AwaitStep.from_wire(data))
        elif msg_type == "done":
            self._emit("done", Done.from_wire(data))
        elif msg_type == "log":
            self._emit("log", LogEntry.from_wire(data))
        elif msg_type == "memory":
            request_id = data.get("requestId")
            with self._lock:
                waiter = (
                    self._memory_waiters.pop(request_id, None) if request_id else None
                )
            if waiter is not None:
                episodes = [Episode.from_wire(e) for e in (data.get("episodes") or [])]
                waiter(episodes, data.get("error"))
            else:
                # Uncorrelated/duplicate memory line -- surface it rather than dropping silently.
                self._emit("parse_error", line)
        else:
            self._emit("parse_error", line)

    def _emit(self, event: str, *args: Any) -> None:
        # A buggy listener must not kill the reader thread (which would hang run_task); isolate it.
        for listener in list(self._listeners[event]):
            try:
                listener(*args)
            except Exception as exc:  # pragma: no cover - defensive
                print(f"kriya: listener for {event!r} raised: {exc}", file=sys.stderr)


# -- high-level driver -------------------------------------------------------

# A state value is either a dict snapshot or a zero-arg callable returning the current snapshot
# (so handlers can mutate app state and the latest is sent back each step).
StateLike = Union[Dict[str, Any], Callable[[], Dict[str, Any]]]

# A manual dispatch returns the full result message (it owns the refreshed state).
DispatchFn = Callable[[ActionRequest], ActionResultMsg]
ApproveFn = Callable[[ApprovalRequest], bool]
StepFn = Callable[[AwaitStep], bool]
LogFn = Callable[[LogEntry], None]


def run_task(
    host: Host,
    *,
    goal: str,
    state: StateLike,
    tools: Optional[List[Dict[str, Any]]] = None,
    registry: Optional[Registry] = None,
    dispatch: Optional[DispatchFn] = None,
    approve: Optional[ApproveFn] = None,
    on_step: Optional[StepFn] = None,
    on_log: Optional[LogFn] = None,
    resume: bool = False,
    step_mode: bool = False,
    agent_id: Optional[str] = None,
    user_id: Optional[str] = None,
    timeout: Optional[float] = None,
) -> Done:
    """Drive a single run to completion and return the :class:`Done` summary. Blocks the calling
    thread (the host's reader thread does the work).

    Two ways to execute actions:

    * **Registry-driven (recommended):** pass ``registry=`` and a ``state`` dict/callable. ``tools``
      defaults to ``registry.tool_schemas()`` and each cleared action is validated and run through
      the registry; the current ``state`` is sent back after each step.
    * **Manual:** pass ``dispatch=`` -- a ``Callable[[ActionRequest], ActionResultMsg]`` that runs
      the action and returns the result + refreshed state itself (mirrors ``kriya-sidecar``'s
      ``dispatch``).

    ``approve`` decides guarded actions (default: deny -- the safe choice). ``on_step`` gates
    step-mode pauses (default: advance, so the helper never hangs).
    """
    if dispatch is None and registry is None:
        raise ValueError("run_task needs either registry= or dispatch=")

    def current_state() -> Dict[str, Any]:
        return state() if callable(state) else state

    tools_list = (
        tools
        if tools is not None
        else (registry.tool_schemas() if registry is not None else [])
    )

    if dispatch is not None:
        dispatch_fn: DispatchFn = dispatch
    else:
        assert registry is not None

        def dispatch_fn(req: ActionRequest) -> ActionResultMsg:
            ctx = ActionContext(caller="agent", step_id=req.step_id)
            result = registry.dispatch_action(req.action_id, req.params, ctx)
            return ActionResultMsg(
                step_id=req.step_id,
                success=result.success,
                state=current_state(),
                data=result.data,
                error=result.error,
            )

    done_box: Dict[str, Done] = {}
    done_event = threading.Event()

    def on_action(req: ActionRequest) -> None:
        try:
            result = dispatch_fn(req)
        except Exception as exc:
            result = ActionResultMsg(
                step_id=req.step_id,
                success=False,
                state=current_state(),
                error=str(exc),
            )
        host.send_action_result(result)

    def on_approval(req: ApprovalRequest) -> None:
        decide = approve or (lambda _r: False)
        try:
            approved = bool(decide(req))
        except Exception:
            approved = False  # any failure denies (safe by default)
        host.send_approval(ApprovalResponse(step_id=req.step_id, approved=approved))

    def on_await(ev: AwaitStep) -> None:
        decide = on_step or (lambda _e: True)  # no on_step -> advance (never hang)
        try:
            proceed = bool(decide(ev))
        except Exception:
            proceed = False  # safe: stop
        host.send_step_advance(StepAdvance(gate_id=ev.gate_id, proceed=proceed))

    def on_done(d: Done) -> None:
        done_box["done"] = d
        done_event.set()

    listeners = [
        ("action", on_action),
        ("approval", on_approval),
        ("await_step", on_await),
        ("done", on_done),
    ]
    if on_log is not None:
        listeners.append(("log", on_log))
    for event, listener in listeners:
        host.on(event, listener)

    try:
        host.start(
            StartRequest(
                goal=goal,
                state=current_state(),
                tools=tools_list,
                resume=resume,
                step_mode=step_mode,
                agent_id=agent_id,
                user_id=user_id,
            )
        )
        if not done_event.wait(timeout):
            raise TimeoutError("run_task: host did not finish in time")
        return done_box["done"]
    finally:
        # Detach our listeners so the same Host can drive another run cleanly.
        for event, listener in listeners:
            host.off(event, listener)


__all__ = ["Host", "run_task", "StateLike"]
