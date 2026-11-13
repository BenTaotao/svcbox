import { useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";
import type { RedisConfigInput, RedisConfigView } from "../types";

export function RedisConfigForm({ instanceId }: { instanceId: string }) {
  const { toast, runAction, isPending } = useApp();
  const [view, setView] = useState<RedisConfigView | null>(null);
  const [bind, setBind] = useState("127.0.0.1");
  const [port, setPort] = useState(6379);
  const [password, setPassword] = useState("");
  const [clearPassword, setClearPassword] = useState(false);
  const [appendonly, setAppendonly] = useState(false);

  useEffect(() => {
    call<RedisConfigView>("get_redis_config", { instanceId })
      .then((cfg) => {
        setView(cfg);
        setBind(cfg.bind);
        setPort(cfg.port);
        setAppendonly(cfg.appendonly);
      })
      .catch((e: unknown) => toast(e instanceof Error ? e.message : String(e), "error"));
  }, [instanceId, toast]);

  const save = async () => {
    const input: RedisConfigInput = {
      bind,
      port,
      requirepass: clearPassword ? "" : password.trim() ? password : null,
      appendonly,
    };
    const result = await runAction(
      `redis-config-${instanceId}`,
      () => call<RedisConfigView>("update_redis_config", { instanceId, input }),
      "Redis 配置已保存",
    );
    if (result) {
      setView(result);
      setPassword("");
      setClearPassword(false);
      if (result.requiresRestart) {
        toast("bind / port / 密码 / AOF 变更需要重启 Redis 后生效", "info");
      }
    }
  };

  if (!view) return <div className="muted">加载中...</div>;

  return (
    <section className="panel">
      <h3>Redis 配置</h3>
      <p className="muted small">
        修改 bind、port、密码或 AOF 都需要重启实例后生效，保存时会先写文件并做结构校验。
      </p>
      <div className="form-grid">
        <label>
          bind
          <input value={bind} onChange={(e) => setBind(e.target.value)} />
        </label>
        <label>
          port
          <input
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(e) => setPort(Number(e.target.value))}
          />
        </label>
        <label>
          访问密码
          <input
            type="password"
            value={password}
            placeholder={view.hasPassword ? "已设置，留空保持不变" : "空 = 无密码"}
            onChange={(e) => setPassword(e.target.value)}
          />
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={clearPassword}
            onChange={(e) => setClearPassword(e.target.checked)}
          />
          清除现有密码
        </label>
        <label className="check-row">
          <input
            type="checkbox"
            checked={appendonly}
            onChange={(e) => setAppendonly(e.target.checked)}
          />
          开启 AOF 持久化
        </label>
      </div>
      {bind === "0.0.0.0" && (
        <div className="inline-warning danger-box">
          0.0.0.0 会监听所有网卡，局域网内其他机器可直接访问 Redis。仅在隔离环境使用，并务必设置密码。
        </div>
      )}
      <div className="row-end">
        <button
          className="btn primary"
          disabled={isPending(`redis-config-${instanceId}`)}
          onClick={() => void save()}
        >
          保存配置
        </button>
      </div>
    </section>
  );
}
