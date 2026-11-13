import { useEffect, useMemo, useState } from "react";
import { call } from "../api";
import { AdvancedConfigEditor } from "../components/AdvancedConfigEditor";
import { HealthBadge } from "../components/HealthBadge";
import { KindBadge } from "../components/KindBadge";
import { LogViewer } from "../components/LogViewer";
import { Modal } from "../components/Modal";
import { MySQLPanel } from "../components/MySQLPanel";
import { RedisConfigForm } from "../components/RedisConfigForm";
import { SiteEditor } from "../components/SiteEditor";
import { useApp } from "../store/appStore";
import type { InstanceView } from "../types";

type Tab = "base" | "logs" | "nginx" | "mysql" | "redis" | "raw";

export function InstanceDetail() {
  const {
    instances,
    selectedId,
    navigate,
    refreshInstances,
    runAction,
    isPending,
  } = useApp();
  const [tab, setTab] = useState<Tab>("base");
  const [fetched, setFetched] = useState<InstanceView | null>(null);
  const [removeOpen, setRemoveOpen] = useState(false);
  const [deleteData, setDeleteData] = useState(true);

  const instance = useMemo(
    () => instances.find((i) => i.id === selectedId) ?? fetched,
    [instances, selectedId, fetched],
  );

  useEffect(() => {
    setTab("base");
    setFetched(null);
  }, [selectedId]);

  useEffect(() => {
    if (!selectedId) return;
    if (instances.some((i) => i.id === selectedId)) return;
    let disposed = false;
    call<InstanceView>("get_instance", { id: selectedId })
      .then((value) => {
        if (!disposed) setFetched(value);
      })
      .catch(() => {
        if (!disposed) setFetched(null);
      });
    return () => {
      disposed = true;
    };
  }, [selectedId, instances]);

  if (!instance) {
    return (
      <div className="page">
        <button className="btn ghost" onClick={() => navigate("dashboard")}>
          返回仪表盘
        </button>
        <div className="empty-hint">实例不存在或已被删除。</div>
      </div>
    );
  }

  const active = ["running", "warningPortNotListen", "errorLogHasError"].includes(
    instance.health,
  );

  const act = async (command: "start" | "stop" | "restart") => {
    const result = await runAction(`${command}-${instance.id}`, () =>
      call<InstanceView>(`${command}_instance`, { id: instance.id }),
    );
    if (result) await refreshInstances(true);
  };

  const removeInstance = async () => {
    const result = await runAction(
      "remove-instance",
      () =>
        call<void>("remove_instance", {
          id: instance.id,
          deleteData,
        }),
    );
    if (result !== undefined) {
      setRemoveOpen(false);
      navigate("dashboard");
      await refreshInstances(true);
    }
  };

  const tabs: Array<{ key: Tab; label: string }> = [
    { key: "base", label: "基础信息" },
    { key: "logs", label: "日志" },
  ];
  if (instance.kind === "nginx") tabs.push({ key: "nginx", label: "站点与 HTTPS" });
  if (instance.kind === "mysql") tabs.push({ key: "mysql", label: "数据库与账号" });
  if (instance.kind === "redis") tabs.push({ key: "redis", label: "Redis 配置" });
  tabs.push({ key: "raw", label: "原始配置" });

  return (
    <div className="page instance-detail">
      <header className="page-head">
        <div className="detail-title">
          <button className="btn small ghost" onClick={() => navigate("dashboard")}>
            ← 返回
          </button>
          <KindBadge kind={instance.kind} withName />
          <h1>{instance.name}</h1>
          <HealthBadge status={instance.health} />
        </div>
        <div className="row-actions">
          {active ? (
            <>
              <button
                className="btn ghost"
                disabled={isPending(`stop-${instance.id}`)}
                onClick={() => void act("stop")}
              >
                停止
              </button>
              <button
                className="btn"
                disabled={isPending(`restart-${instance.id}`)}
                onClick={() => void act("restart")}
              >
                重启
              </button>
            </>
          ) : (
            <button
              className="btn primary"
              disabled={isPending(`start-${instance.id}`)}
              onClick={() => void act("start")}
            >
              启动
            </button>
          )}
          <button className="btn danger-ghost" onClick={() => setRemoveOpen(true)}>
            删除实例
          </button>
        </div>
      </header>

      <nav className="tabs">
        {tabs.map((item) => (
          <button
            key={item.key}
            className={`tab ${tab === item.key ? "active" : ""}`}
            onClick={() => setTab(item.key)}
          >
            {item.label}
          </button>
        ))}
      </nav>

      {tab === "base" && (
        <section className="panel">
          <h3>运行信息</h3>
          <div className="info-grid">
            <div>
              <span className="info-label">类型</span>
              <span className="mono">{instance.kind}</span>
            </div>
            <div>
              <span className="info-label">版本</span>
              <span className="mono">{instance.version}</span>
            </div>
            <div>
              <span className="info-label">HTTP / 主端口</span>
              <span className="mono">{instance.port}</span>
            </div>
            {instance.kind === "nginx" && (
              <div>
                <span className="info-label">HTTPS 端口</span>
                <span className="mono">{instance.httpsPort}</span>
              </div>
            )}
            <div>
              <span className="info-label">PID</span>
              <span className="mono">{instance.pid ?? "-"}</span>
            </div>
            <div>
              <span className="info-label">健康状态</span>
              <HealthBadge status={instance.health} />
            </div>
            <div className="span-2">
              <span className="info-label">访问地址</span>
              <span className="mono">{instance.urls.join("  ")}</span>
            </div>
            <div className="span-2">
              <span className="info-label">实例目录</span>
              <span className="mono muted selectable">{instance.dir}</span>
            </div>
          </div>
          {instance.kind === "mysql" && (
            <p className="muted small">
              root 密码加密保存在 SQLite 中，只在创建时明文显示一次。
            </p>
          )}
        </section>
      )}

      {tab === "logs" && (
        <section className="panel no-pad">
          <LogViewer instanceId={instance.id} />
        </section>
      )}

      {tab === "nginx" && <SiteEditor instanceId={instance.id} />}
      {tab === "mysql" && <MySQLPanel instanceId={instance.id} />}
      {tab === "redis" && <RedisConfigForm instanceId={instance.id} />}
      {tab === "raw" && <AdvancedConfigEditor instanceId={instance.id} />}

      {removeOpen && (
        <Modal title={`删除实例 ${instance.name}`} onClose={() => setRemoveOpen(false)}>
          <div className="inline-warning danger-box">
            实例会先被停止并删除数据库记录。删除后无法恢复，请确认不再需要该环境。
          </div>
          <label className="check-row">
            <input
              type="checkbox"
              checked={deleteData}
              onChange={(e) => setDeleteData(e.target.checked)}
            />
            同时删除实例目录（配置、MySQL 数据、Redis 数据、证书）
          </label>
          <div className="muted small mono">{instance.dir}</div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setRemoveOpen(false)}>
              取消
            </button>
            <button
              className="btn danger"
              disabled={isPending("remove-instance")}
              onClick={() => void removeInstance()}
            >
              删除实例
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
