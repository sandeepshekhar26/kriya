"""The agent-loop wire protocol the Python binding speaks to ``kriya-host`` over stdio.

Each line on the wire is a JSON object ``{"type": ..., "data": {...}}``. The host emits one object
per line on stdout (host -> app) and reads one per line on stdin (app -> host). The ``data`` fields
are **camelCase** on the wire (the Rust structs use ``#[serde(rename_all = "camelCase")]``), except
``done`` whose fields are already lowercase. These dataclasses use idiomatic snake_case and convert
at the edge via ``from_wire`` / ``to_wire``, so app code never juggles casing.

Mirrors kriya-core's ``protocol.ts`` and the Rust ``crates/kriya/src/protocol.rs`` / ``sidecar.rs``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


# -- host -> app -------------------------------------------------------------


@dataclass
class ActionRequest:
    """host -> app: run this action and reply with an :class:`ActionResultMsg` (same ``step_id``)."""

    step_id: str
    action_id: str
    params: Dict[str, Any]
    reasoning: str = ""

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "ActionRequest":
        return cls(
            step_id=d["stepId"],
            action_id=d["actionId"],
            params=d.get("params") or {},
            reasoning=d.get("reasoning", ""),
        )


@dataclass
class ApprovalRequest:
    """host -> app: this guarded action needs a human's go-ahead. Reply with
    :class:`ApprovalResponse` (same ``step_id``)."""

    step_id: str
    action_id: str
    params: Dict[str, Any]
    reasoning: str = ""

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "ApprovalRequest":
        return cls(
            step_id=d["stepId"],
            action_id=d["actionId"],
            params=d.get("params") or {},
            reasoning=d.get("reasoning", ""),
        )


@dataclass
class AwaitStep:
    """host -> app: paused before a step (step-mode only). Reply with :class:`StepAdvance`."""

    gate_id: str
    step_number: int
    last_action_id: Optional[str] = None
    last_success: Optional[bool] = None

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "AwaitStep":
        return cls(
            gate_id=d["gateId"],
            step_number=d.get("stepNumber", 0),
            last_action_id=d.get("lastActionId"),
            last_success=d.get("lastSuccess"),
        )


@dataclass
class Done:
    """host -> app: the run finished. (Wire fields are already lowercase: ``summary``/``steps``.)"""

    summary: str
    steps: int

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "Done":
        return cls(summary=d.get("summary", ""), steps=d.get("steps", 0))


@dataclass
class LogEntry:
    """host -> app: inspector/governance telemetry (reasoning, decisions, errors)."""

    level: str
    message: str
    step_id: Optional[str] = None
    detail: Any = None

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "LogEntry":
        return cls(
            level=d.get("level", "info"),
            message=d.get("message", ""),
            step_id=d.get("stepId"),
            detail=d.get("detail"),
        )


@dataclass
class Episode:
    """One recorded action from the host's durable episodic memory (newest-first).

    ``params`` is a JSON-encoded string exactly as the host signed it (parse it if you need the
    object). Mirrors the Rust ``kriya::memory::Episode`` and kriya-core's ``Episode``.
    """

    ts_ms: int
    action_id: str
    params: str
    success: bool
    reasoning: str
    signature: str
    run_id: str
    goal: str

    @classmethod
    def from_wire(cls, d: Dict[str, Any]) -> "Episode":
        return cls(
            ts_ms=d.get("tsMs", 0),
            action_id=d.get("actionId", ""),
            params=d.get("params", ""),
            success=bool(d.get("success", False)),
            reasoning=d.get("reasoning", ""),
            signature=d.get("signature", ""),
            run_id=d.get("runId", ""),
            goal=d.get("goal", ""),
        )


# -- app -> host -------------------------------------------------------------


@dataclass
class StartRequest:
    """app -> host: begin an autonomous run."""

    goal: str
    state: Dict[str, Any]
    tools: List[Dict[str, Any]] = field(default_factory=list)
    resume: bool = False
    step_mode: bool = False
    agent_id: Optional[str] = None
    user_id: Optional[str] = None

    def to_wire(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {
            "goal": self.goal,
            "state": self.state,
            "tools": self.tools,
            "resume": self.resume,
            "stepMode": self.step_mode,
        }
        if self.agent_id is not None:
            out["agentId"] = self.agent_id
        if self.user_id is not None:
            out["userId"] = self.user_id
        return out


@dataclass
class ActionResultMsg:
    """app -> host: the result of an action the host asked you to run, plus refreshed state."""

    step_id: str
    success: bool
    state: Dict[str, Any]
    data: Any = None
    error: Optional[str] = None

    def to_wire(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {
            "stepId": self.step_id,
            "success": self.success,
            "state": self.state,
        }
        if self.data is not None:
            out["data"] = self.data
        if self.error is not None:
            out["error"] = self.error
        return out


@dataclass
class ApprovalResponse:
    """app -> host: a human's decision on a pending approval."""

    step_id: str
    approved: bool

    def to_wire(self) -> Dict[str, Any]:
        return {"stepId": self.step_id, "approved": self.approved}


@dataclass
class StepAdvance:
    """app -> host: advance (``proceed=True``) or stop (``False``) a step-mode run."""

    gate_id: str
    proceed: bool

    def to_wire(self) -> Dict[str, Any]:
        return {"gateId": self.gate_id, "proceed": self.proceed}


__all__ = [
    "ActionRequest",
    "ApprovalRequest",
    "AwaitStep",
    "Done",
    "LogEntry",
    "Episode",
    "StartRequest",
    "ActionResultMsg",
    "ApprovalResponse",
    "StepAdvance",
]
