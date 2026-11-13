import type { DownloadProgress } from "../types";
import { Modal } from "./Modal";

export function DownloadProgressModal({
  progress,
  onClose,
}: {
  progress: DownloadProgress | null;
  onClose: () => void;
}) {
  if (!progress) return null;
  const done = progress.percent >= 100 || progress.stage.includes("完成");
  return (
    <Modal
      title={`${progress.kind} ${progress.version}`}
      onClose={onClose}
      closable={done}
    >
      <div className="progress-stage">
        <span>{progress.stage}</span>
        <span className="mono">{progress.percent.toFixed(1)}%</span>
      </div>
      <div className="progress-track">
        <div className="progress-fill" style={{ width: `${Math.min(progress.percent, 100)}%` }} />
      </div>
      <div className="muted small mono">
        {progress.downloaded} / {progress.total ?? "?"} bytes
      </div>
      <p className="muted small">
        安装包从官方地址下载，落盘前强制校验 sha256，校验失败会自动删除缓存。
      </p>
    </Modal>
  );
}
