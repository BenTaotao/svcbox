import { useEffect, useMemo, useState } from "react";
import { call } from "../api";
import { DoctorModal } from "../components/DoctorModal";
import { InstanceCard } from "../components/InstanceCard";
import { Modal } from "../components/Modal";
import { useApp } from "../store/appStore";
import type {
  CreateInstanceOptions,
  CreateInstanceResult,
  ServiceKind,
  VersionInfo,
} from "../types";

const DEFAULT_PORTS: Record<ServiceKind, number> = {
  nginx: 8080,
  mysql: 3306,
  redis: 6379,
};

export function Dashboard() {
  const {
    instances,
    versions,
    loading,
    refreshInstances,
    refreshVersions,
    navigate,
    runAction,
    isPending,
    toast,
  } = useApp();

  const [createOpen, setCreateOpen] = useState(false);
  const [doctorOpen, setDoctorOpen] = useState(false);
  const [kind, setKind] = useState<ServiceKind>("nginx");
  const [name, setName] = useState("");
  const [version, setVersion] = useState("");
  const [port, setPort] = useState(DEFAULT_PORTS.nginx);
  const [rootPassword, setRootPassword] = useState("");
  const [createdPassword, setCreatedPassword] = useState<string | null>(null);

  const installed = useMemo(
    () => versions.filter((v) => v.kind === kind && v.installed),
    [versions, kind],
  );

  useEffect(() => {
    const first = installed[0];
    if (first && !installed.some((v) => v.version === version)) {
      setVersion(first.version);
    }
  }, [installed, version]);

  useEffect(() => {
    void refreshVersions();
  }, [refreshVersions]);

  const stats = useMemo(() => {
    const running = instances.filter((i) =>
      ["running", "warningPortNotListen", "errorLogHasError"].includes(i.health),
    ).length;
    const crashed = instances.filter((i) => i.health === "crashed").length;
    return {
      total: instances.length,
      running,
      crashed,
      stopped: instances.length - running - crashed,
    };
  }, [instances]);

  const changeKind = (next: ServiceKind) => {
    setKind(next);
    setPort(DEFAULT_PORTS[next]);
    setVersion("");
  };

  const createInstance = async () => {
    const opts: CreateInstanceOptions = {
      kind,
      name,
      version,
      port,
      rootPassword: kind === "mysql" && rootPassword ? rootPassword : null,
    };
    const result = await runAction(
      "create-instance",
      () => call<CreateInstanceResult>("create_instance", { opts }),
    );
    if (!result) return;
    setCreateOpen(false);
    setName("");
    setRootPassword("");
    if (result.initialRootPassword) {
      setCreatedPassword(result.initialRootPassword);
    } else {
      toast("实例已创建", "success");
    }
    await refreshInstances();
  };

  const startAll = async () => {
    const errors = await runAction("start-all", () =>
      call<string[]>("start_all_instances"),
    );
    await refreshInstances(true);
    if (errors && errors.length > 0) {
      errors.forEach((e) => toast(e, "error"));
    } else if (errors) {
      toast("已尝试启动所有实例", "success");
    }
  };

  const stopAll = async () => {
    const errors = await runAction("stop-all", () =>
      call<string[]>("stop_all_instances"),
    );
    await refreshInstances(true);
    if (errors && errors.length > 0) {
      errors.forEach((e) => toast(e, "error"));
    } else if (errors) {
      toast("已停止所有实例", "success");
    }
  };

  return (
    <div className="page dashboard-page">
      <header className="page-head">
        <div>
          <h1>仪表盘</h1>
          <p className="muted">隔离的多实例环境，一键启停，全部进程都由 JobObject 托管。</p>
        </div>
        <div className="row-actions">
          <button className="btn ghost" onClick={() => setDoctorOpen(true)}>
            环境自检
          </button>
          <button
            className="btn ghost"
            disabled={isPending("stop-all") || stats.running === 0}
            onClick={() => void stopAll()}
          >
            全部停止
          </button>
          <button
            className="btn ghost"
            disabled={isPending("start-all")}
            onClick={() => void startAll()}
          >
            全部启动
          </button>
          <button className="btn primary" onClick={() => setCreateOpen(true)}>
            新建实例
          </button>
        </div>
      </header>

      <section className="stat-row">
        <div className="stat">
          <span className="stat-value">{stats.total}</span>
          <span className="stat-label">实例总数</span>
        </div>
        <div className="stat stat-ok">
          <span className="stat-value">{stats.running}</span>
          <span className="stat-label">运行中</span>
        </div>
        <div className="stat stat-bad">
          <span className="stat-value">{stats.crashed}</span>
          <span className="stat-label">异常</span>
        </div>
        <div className="stat">
          <span className="stat-value">{stats.stopped}</span>
          <span className="stat-label">已停止</span>
        </div>
      </section>

      {loading && instances.length === 0 ? (
        <div className="empty-hint">正在加载实例...</div>
      ) : instances.length === 0 ? (
        <div className="empty-panel">
          <div className="empty-title">还没有实例</div>
          <p className="muted">
            先在版本仓库安装 Nginx / MySQL / Redis，然后创建第一个隔离实例。
          </p>
          <div className="row-actions center">
            <button className="btn ghost" onClick={() => navigate("versions")}>
              去版本仓库
            </button>
            <button className="btn primary" onClick={() => setCreateOpen(true)}>
              新建实例
            </button>
          </div>
        </div>
      ) : (
        <section className="instance-grid">
          {instances.map((instance) => (
            <InstanceCard key={instance.id} instance={instance} />
          ))}
        </section>
      )}

      {createOpen && (
        <Modal title="新建实例" onClose={() => setCreateOpen(false)}>
          <div className="form-grid one-col">
            <label>
              服务类型
              <select
                value={kind}
                onChange={(e) => changeKind(e.target.value as ServiceKind)}
              >
                <option value="nginx">Nginx</option>
                <option value="mysql">MySQL</option>
                <option value="redis">Redis</option>
              </select>
            </label>
            <label>
              实例名（同时作为目录名）
              <input
                value={name}
                placeholder="dev"
                onChange={(e) => setName(e.target.value)}
              />
            </label>
            <label>
              版本
              <select value={version} onChange={(e) => setVersion(e.target.value)}>
                {installed.length === 0 && <option value="">暂无已安装版本</option>}
                {installed.map((v: VersionInfo) => (
                  <option key={v.version} value={v.version}>
                    {v.version}
                  </option>
                ))}
              </select>
            </label>
            <label>
              端口
              <input
                type="number"
                min={1}
                max={65535}
                value={port}
                onChange={(e) => setPort(Number(e.target.value))}
              />
            </label>
            {kind === "mysql" && (
              <label>
                root 初始密码（留空自动生成）
                <input
                  type="password"
                  value={rootPassword}
                  placeholder="留空则生成 16 位随机密码"
                  onChange={(e) => setRootPassword(e.target.value)}
                />
              </label>
            )}
            {installed.length === 0 && (
              <div className="inline-warning">
                当前类型还没有已安装版本。
                <button className="link-btn" onClick={() => navigate("versions")}>
                  去安装
                </button>
              </div>
            )}
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setCreateOpen(false)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={
                !name.trim() || !version || installed.length === 0 || isPending("create-instance")
              }
              onClick={() => void createInstance()}
            >
              创建
            </button>
          </div>
        </Modal>
      )}

      {createdPassword && (
        <Modal title="MySQL root 初始密码" onClose={() => setCreatedPassword(null)}>
          <div className="inline-warning">
            该密码只显示这一次，请立即保存。后续可在实例的 MySQL 页签中重置。
          </div>
          <div className="password-display mono">{createdPassword}</div>
          <div className="modal-actions">
            <button
              className="btn ghost"
              onClick={() => {
                void navigator.clipboard
                  .writeText(createdPassword)
                  .catch(() => toast("复制失败，请手动选择密码", "error"));
              }}
            >
              复制
            </button>
            <button className="btn primary" onClick={() => setCreatedPassword(null)}>
              我已保存
            </button>
          </div>
        </Modal>
      )}

      {doctorOpen && <DoctorModal onClose={() => setDoctorOpen(false)} />}
    </div>
  );
}
