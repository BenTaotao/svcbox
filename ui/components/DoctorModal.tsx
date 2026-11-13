import { useEffect, useState } from "react";
import { call } from "../api";
import type { DoctorReport } from "../types";
import { Modal } from "./Modal";

export function DoctorModal({ onClose }: { onClose: () => void }) {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    call<DoctorReport>("get_doctor")
      .then(setReport)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  return (
    <Modal title="环境自检" onClose={onClose} wide>
      {error && <div className="inline-error">{error}</div>}
      {!report && !error && <div className="muted">正在检查...</div>}
      {report && (
        <>
          <div className={`doctor-summary ${report.ok ? "ok" : "bad"}`}>
            {report.ok ? "自检通过" : "发现异常，请按提示处理"}
          </div>
          <ul className="doctor-list">
            {report.checks.map((check) => (
              <li key={check.name} className={check.ok ? "ok" : "bad"}>
                <span className="doctor-mark">{check.ok ? "✓" : "!"}</span>
                <span className="doctor-name">{check.name}</span>
                <span className="doctor-detail muted">{check.detail}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </Modal>
  );
}
