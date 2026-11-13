export interface ConfirmOptions {
  title: string;
  message: string;
  danger?: boolean;
  confirmText?: string;
}

export interface ConfirmState extends ConfirmOptions {
  resolve: (ok: boolean) => void;
}

export function ConfirmModal({ state }: { state: ConfirmState | null }) {
  if (!state) return null;
  const finish = (ok: boolean) => {
    state.resolve(ok);
  };
  return (
    <div className="modal-mask" onMouseDown={() => finish(false)}>
      <div className="modal confirm-modal" onMouseDown={(e) => e.stopPropagation()}>
        <h3 className={state.danger ? "danger-text" : ""}>{state.title}</h3>
        <p className="confirm-message">{state.message}</p>
        <div className="modal-actions">
          <button className="btn ghost" onClick={() => finish(false)}>
            取消
          </button>
          <button
            className={`btn ${state.danger ? "danger" : "primary"}`}
            onClick={() => finish(true)}
            autoFocus
          >
            {state.confirmText ?? "确定"}
          </button>
        </div>
      </div>
    </div>
  );
}
