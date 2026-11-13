import type { HealthStatus } from "../types";

const LABELS: Record<HealthStatus, string> = {
  stopped: "已停止",
  running: "运行中",
  warningPortNotListen: "端口未监听",
  errorLogHasError: "日志报错",
  crashed: "已崩溃",
};

export function healthLabel(status: HealthStatus): string {
  return LABELS[status];
}

export function HealthBadge({ status }: { status: HealthStatus }) {
  return <span className={`health-badge health-${status}`}>{LABELS[status]}</span>;
}
