import { useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";
import type { NginxGlobalConfig } from "../types";

const EMPTY: NginxGlobalConfig = {
  workerProcesses: 1,
  workerConnections: 1024,
  keepaliveTimeout: 65,
  gzip: true,
  clientMaxBodySize: "64m",
  httpsPort: 8443,
};

export function NginxGlobalForm({ instanceId }: { instanceId: string }) {
  const { runAction, isPending } = useApp();
  const [cfg, setCfg] = useState<NginxGlobalConfig>(EMPTY);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    call<NginxGlobalConfig>("get_nginx_global", { instanceId })
      .then((value) => {
        setCfg(value);
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, [instanceId]);

  const update = <K extends keyof NginxGlobalConfig>(
    key: K,
    value: NginxGlobalConfig[K],
  ) => setCfg((prev) => ({ ...prev, [key]: value }));

  const save = async () => {
    await runAction(
      `nginx-global-${instanceId}`,
      () => call<void>("update_nginx_global", { instanceId, cfg }),
      "Nginx 全局配置已保存",
    );
  };

  if (!loaded) return <div className="muted">加载中...</div>;

  return (
    <section className="panel">
      <h3>Nginx 全局配置</h3>
      <p className="muted small">
        保存后先执行 nginx -t，通过则热重载；worker_processes 等参数会随 reload 生效。
      </p>
      <div className="form-grid">
        <label>
          worker_processes
          <input
            type="number"
            min={1}
            max={64}
            value={cfg.workerProcesses}
            onChange={(e) => update("workerProcesses", Number(e.target.value))}
          />
        </label>
        <label>
          worker_connections
          <input
            type="number"
            min={1}
            max={65535}
            value={cfg.workerConnections}
            onChange={(e) => update("workerConnections", Number(e.target.value))}
          />
        </label>
        <label>
          keepalive_timeout
          <input
            type="number"
            min={0}
            max={3600}
            value={cfg.keepaliveTimeout}
            onChange={(e) => update("keepaliveTimeout", Number(e.target.value))}
          />
        </label>
        <label>
          client_max_body_size
          <input
            value={cfg.clientMaxBodySize}
            onChange={(e) => update("clientMaxBodySize", e.target.value)}
          />
        </label>
        <label>
          HTTPS 端口
          <input
            type="number"
            min={1}
            max={65535}
            value={cfg.httpsPort}
            onChange={(e) => update("httpsPort", Number(e.target.value))}
          />
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={cfg.gzip}
            onChange={(e) => update("gzip", e.target.checked)}
          />
          开启 gzip
        </label>
      </div>
      <div className="row-end">
        <button
          className="btn primary"
          disabled={isPending(`nginx-global-${instanceId}`)}
          onClick={() => void save()}
        >
          保存全局配置
        </button>
      </div>
    </section>
  );
}
