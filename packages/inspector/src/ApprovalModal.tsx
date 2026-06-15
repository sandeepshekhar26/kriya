/**
 * The framework's approval-modal component. Renders nothing when there's no pending
 * request; renders a focused dialog otherwise. The host loop pauses on a per-step
 * channel waiting for `onApprove` / `onDeny` to be dispatched.
 */

import type { AgentApprovalRequest } from "kriya-core";

export interface ApprovalModalProps {
  request: AgentApprovalRequest | null;
  onApprove: () => void;
  onDeny: () => void;
}

export function ApprovalModal({ request, onApprove, onDeny }: ApprovalModalProps) {
  if (!request) return null;

  return (
    <div className="an-modal-backdrop" role="dialog" aria-modal="true">
      <div className="an-modal">
        <h3>Approval required</h3>
        <p className="an-modal-sub">
          The agent wants to run a guarded action. The host is paused until you decide.
        </p>
        <div className="an-modal-action">
          <code className="an-modal-action-id">{request.actionId}</code>
          <pre>{JSON.stringify(request.params, null, 2)}</pre>
          {request.reasoning && (
            <p className="an-modal-reason">“{request.reasoning}”</p>
          )}
        </div>
        <div className="an-modal-buttons">
          <button onClick={onDeny}>Deny</button>
          <button className="an-primary an-danger" onClick={onApprove}>
            Approve
          </button>
        </div>
      </div>
    </div>
  );
}
