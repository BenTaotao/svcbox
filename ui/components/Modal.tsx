import type { ReactNode } from "react";

export function Modal({
  title,
  onClose,
  children,
  wide,
  closable = true,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
  closable?: boolean;
}) {
  return (
    <div className="modal-mask" onMouseDown={closable ? onClose : undefined}>
      <div
        className={`modal ${wide ? "wide" : ""}`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h3>{title}</h3>
          {closable && (
            <button className="icon-btn" onClick={onClose} aria-label="关闭">
              ✕
            </button>
          )}
        </header>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
