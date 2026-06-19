"""examples/note_app_host.py — host the kriya governed agent runtime from plain Python.

The Python mirror of examples/node-sidecar-host (the embedded sidecar path, roadmap R3). Your own
process spawns the `kriya-host` binary and drives a governed run over stdio. The agent loop, the
policy engine, human approval, the budget, and the signed audit log all live inside that separate
process — your process only ever runs the typed actions the host has already cleared, the same
handlers a button click would call.

Run it (deterministic, no API key — uses demo-script.json):

    cd bindings/python
    (cd ../../apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)  # once
    python examples/note_app_host.py
    # or point at your own build:  KRIYA_HOST_BIN=/path/to/kriya-host python examples/note_app_host.py
"""

import os
import sys
from datetime import datetime, timezone
from pathlib import Path

# Run straight from the checkout without `pip install` by adding the package src to the path.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import kriya  # noqa: E402
from kriya import ok, required, string  # noqa: E402

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]


def locate_host() -> str:
    env = os.environ.get("KRIYA_HOST_BIN")
    candidate = (
        env
        if env
        else str(REPO / "apps" / "note-app" / "src-tauri" / "target" / "debug" / "kriya-host")
    )
    if not Path(candidate).exists():
        sys.exit(
            f"\nkriya-host not found at:\n  {candidate}\n\n"
            "Build it first:\n"
            "  (cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)\n"
            "or set KRIYA_HOST_BIN to your own built binary.\n"
        )
    return candidate


# The app's own state + typed actions — the SAME handlers a human button would call. The agent
# never touches this directly; it only proposes an action id + params, and the governed host
# decides whether that proposal is allowed to reach these handlers.
notes: list = []

kriya.register_action(
    id="create_note",
    description="Create a note with a title.",
    parameters={"title": required(string)},
    permissions=["write:notes"],
    handler=lambda p, ctx: (notes.append({"id": len(notes) + 1, "title": p["title"]}), ok(notes[-1]))[1],
)

kriya.register_action(
    id="delete_note",
    description="Delete a note by id.",
    parameters={"id": required(kriya.number)},
    permissions=["delete:notes"],  # policy: delete_* → require_approval (held for a human)
    handler=lambda p, ctx: ok({"deleted": p["id"]}),
)


def main() -> None:
    binary = locate_host()
    host = kriya.Host.spawn(binary, ["--script", str(HERE / "demo-script.json")])

    print("\n=== hosting kriya-host from Python — governance runs in the child process ===\n")

    def approve(req) -> bool:
        print(f'  [APPROVE]  "{req.action_id}" needs a human — granting (reason: {req.reasoning})')
        return True

    done = kriya.run_task(
        host,
        goal="tidy up the notes",
        state=lambda: {"notes": list(notes)},
        registry=kriya.default_registry(),
        approve=approve,
        on_log=lambda e: print(f"             - [{e.level}] {e.message}"),
        timeout=30.0,
    )

    print(f'\n=== done: "{done.summary}" ({done.steps} step(s)) ===')
    print(f"    final notes: {notes}\n")

    # Durable memory over the sidecar protocol — reads the same episodic log Tauri reads.
    episodes = host.recent_memory(5)
    print(f"=== recent_memory(): {len(episodes)} newest episode(s) (persists across runs) ===")
    for e in episodes:
        when = datetime.fromtimestamp(e.ts_ms / 1000, tz=timezone.utc).isoformat()
        flag = "ok " if e.success else "err"
        print(f"    {when}  {e.action_id:<12} {flag}  sig={e.signature[:12]}…")
    print("")

    host.close()


if __name__ == "__main__":
    main()
