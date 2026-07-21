"""A STUB agent (no LLM, no API keys) driving kriya.agents' governed tools the way a LangGraph
ToolNode would call them -- policy -> (approval) -> budget -> a signed receipt per tool call,
produced by the runtime's kriya-govern binary (kriya.agents signs nothing itself).

Run (after `cargo build -p kriya --bin kriya-govern`):
    PYTHONPATH=src python examples/govern_langgraph_stub.py

Then re-verify the receipts it wrote, offline, with the runtime's verifier or the kriya Console.
"""

import os
import tempfile
from pathlib import Path

from kriya.agents import GovernClient, GovernDenied
from kriya.agents.langgraph import govern_tool

_HERE = Path(__file__).resolve().parent


def _find_bin():
    env = os.environ.get("KRIYA_GOVERN_BIN")
    if env and Path(env).exists():
        return env
    for rel in (
        "../../../apps/note-app/src-tauri/target/debug/kriya-govern",
        "../../../crates/kriya/target/debug/kriya-govern",
        "../../../target/debug/kriya-govern",
    ):
        p = (_HERE / rel).resolve()
        if p.exists():
            return str(p)
    return None


def main():
    binary = _find_bin()
    if not binary:
        raise SystemExit("kriya-govern not found — build it: cargo build -p kriya --bin kriya-govern")

    home = Path.home()
    audit_dir = home / ".kriya" / "audit" if (home / ".kriya").exists() else Path("/tmp")
    audit_log = str(audit_dir / "langgraph-py-stub.jsonl")

    policy = os.path.join(tempfile.mkdtemp(prefix="kriya-stub-"), "policy.yaml")
    Path(policy).write_text(
        'rules:\n'
        '  - { action: "web_search", allow: true }\n'
        '  - { action: "delete_files", allow: false }\n'
        '  - { action: "*", allow: true }\n'
        'budget:\n  max_actions_per_minute: 60\n'
    )

    client = GovernClient(
        binary_path=binary,
        policy_path=policy,
        actor="langgraph",
        user=os.environ.get("USER", "demo"),
        audit_log=audit_log,
    )
    print(f"audit log -> {audit_log}\n")

    # LangChain calls tools with keyword args; govern_tool handles that shape.
    web_search = govern_tool(client, "web_search", lambda q: [f'(stub) top result for "{q}"'])
    delete_files = govern_tool(client, "delete_files", lambda glob: f"deleted {glob}")

    print("web_search ->", web_search(q="kriya governance"))
    try:
        delete_files(glob="/**")
        print("delete_files -> (unexpectedly ran)")
    except GovernDenied as e:
        print(f"delete_files -> DENIED by policy ({e.decision}) — no receipt signed, agent adapts")
    print("web_search ->", web_search(q="signed receipts"))

    client.close()
    print(f"\nDone. 2 signed receipts in {audit_log} — re-verify them in the Console.")


if __name__ == "__main__":
    main()
