import { useCallback, useEffect, useRef, useState } from "react";
import { call } from "../api";
import type { LogChunk } from "../types";

const ERROR_RE = /\b(error|fatal|emerg|critical|panic)\b/i;

export function LogViewer({ instanceId }: { instanceId: string }) {
  const [chunk, setChunk] = useState<LogChunk | null>(null);
  const [follow, setFollow] = useState(true);
  const [autoScroll, setAutoScroll] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const boxRef = useRef<HTMLPreElement>(null);

  const load = useCallback(async () => {
    try {
      setChunk(await call<LogChunk>("read_logs", { id: instanceId, lines: 500 }));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [instanceId]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!follow) return;
    const timer = window.setInterval(() => void load(), 2000);
    return () => window.clearInterval(timer);
  }, [follow, load]);

  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [chunk, autoScroll]);

  const lines = chunk?.content.split("\n") ?? [];

  return (
    <div className="log-viewer">
      <div className="log-toolbar">
        <span className="muted small mono log-path">{chunk?.path ?? ""}</span>
        <label className="check-inline">
          <input
            type="checkbox"
            checked={follow}
            onChange={(e) => setFollow(e.target.checked)}
          />
          实时刷新
        </label>
        <label className="check-inline">
          <input
            type="checkbox"
            checked={autoScroll}
            onChange={(e) => setAutoScroll(e.target.checked)}
          />
          自动滚动
        </label>
        <button className="btn small ghost" onClick={() => void load()}>
          刷新
        </button>
      </div>
      {error && <div className="inline-error">{error}</div>}
      <pre className="log-body" ref={boxRef}>
        {lines.length === 0 ? (
          <div className="muted">暂无日志</div>
        ) : (
          lines.map((line, idx) => (
            <div key={idx} className={ERROR_RE.test(line) ? "log-error" : ""}>
              {line || " "}
            </div>
          ))
        )}
      </pre>
    </div>
  );
}
