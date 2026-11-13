import { useCallback, useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";
import type { MysqlUser } from "../types";
import { Modal } from "./Modal";

export function MySQLPanel({ instanceId }: { instanceId: string }) {
  const { toast, confirm, runAction, isPending } = useApp();
  const [dbs, setDbs] = useState<string[]>([]);
  const [users, setUsers] = useState<MysqlUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [dbModal, setDbModal] = useState(false);
  const [dbName, setDbName] = useState("");
  const [charset, setCharset] = useState("utf8mb4");
  const [collate, setCollate] = useState("utf8mb4_unicode_ci");

  const [userModal, setUserModal] = useState(false);
  const [user, setUser] = useState("");
  const [host, setHost] = useState("localhost");
  const [password, setPassword] = useState("");
  const [selectedDbs, setSelectedDbs] = useState<string[]>([]);

  const [pwdTarget, setPwdTarget] = useState<MysqlUser | null>(null);
  const [newPassword, setNewPassword] = useState("");
  const [rootModal, setRootModal] = useState(false);
  const [rootPassword, setRootPassword] = useState("");

  const load = useCallback(async () => {
    try {
      const [dbList, userList] = await Promise.all([
        call<string[]>("list_mysql_dbs", { instanceId }),
        call<MysqlUser[]>("list_mysql_users", { instanceId }),
      ]);
      setDbs(dbList);
      setUsers(userList);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [instanceId]);

  useEffect(() => {
    void load();
  }, [load]);

  const createDb = async () => {
    const result = await runAction(
      "create-db",
      () =>
        call<void>("create_mysql_db", {
          instanceId,
          dbName,
          charset,
          collate,
        }),
      `数据库 ${dbName} 已创建`,
    );
    if (result !== undefined) {
      setDbModal(false);
      setDbName("");
      await load();
    }
  };

  const dropDb = async (name: string) => {
    const ok = await confirm({
      title: `删除数据库 ${name}`,
      message: "数据库内所有表和数据都会被删除，且不可恢复。确认继续？",
      danger: true,
      confirmText: "永久删除",
    });
    if (!ok) return;
    const result = await runAction("drop-db", () =>
      call<void>("drop_mysql_db", { instanceId, dbName: name }),
    );
    if (result !== undefined) {
      toast(`数据库 ${name} 已删除`, "success");
      await load();
    }
  };

  const createUser = async () => {
    const result = await runAction(
      "create-user",
      () =>
        call<void>("create_mysql_user", {
          instanceId,
          user,
          host,
          password,
          dbs: selectedDbs,
        }),
      `账号 ${user}@${host} 已创建`,
    );
    if (result !== undefined) {
      setUserModal(false);
      setUser("");
      setPassword("");
      setSelectedDbs([]);
      await load();
    }
  };

  const alterPassword = async () => {
    if (!pwdTarget) return;
    const result = await runAction(
      "alter-user-password",
      () =>
        call<void>("alter_mysql_user_password", {
          instanceId,
          user: pwdTarget.user,
          host: pwdTarget.host,
          newPassword,
        }),
      "密码已修改",
    );
    if (result !== undefined) {
      setPwdTarget(null);
      setNewPassword("");
    }
  };

  const dropUser = async (target: MysqlUser) => {
    const ok = await confirm({
      title: `删除账号 ${target.user}@${target.host}`,
      message: "账号删除后无法恢复，已授权的数据库数据不会删除。",
      danger: true,
      confirmText: "删除账号",
    });
    if (!ok) return;
    const result = await runAction("drop-user", () =>
      call<void>("drop_mysql_user", {
        instanceId,
        user: target.user,
        host: target.host,
      }),
    );
    if (result !== undefined) {
      toast("账号已删除", "success");
      await load();
    }
  };

  const resetRoot = async () => {
    const ok = await confirm({
      title: "重置 root 密码",
      message:
        "该操作会短暂停止 MySQL，并以 --skip-grant-tables 模式重启完成重置，期间数据库不对其他程序提供服务。确认继续？",
      danger: true,
      confirmText: "开始重置",
    });
    if (!ok) return;
    const result = await runAction(
      "reset-root",
      () =>
        call<void>("reset_mysql_root_password", {
          instanceId,
          newPassword: rootPassword,
        }),
      "root 密码已重置",
    );
    if (result !== undefined) {
      setRootModal(false);
      setRootPassword("");
    }
  };

  if (loading) return <div className="muted">加载中...</div>;

  return (
    <div className="stack">
      {error && (
        <div className="inline-error">
          {error}
          <div className="muted small">MySQL 未运行时无法管理库和账号，请先启动实例。</div>
        </div>
      )}

      <section className="panel">
        <div className="panel-head">
          <div>
            <h3>数据库</h3>
            <p className="muted small">系统库已自动过滤。</p>
          </div>
          <button className="btn small primary" onClick={() => setDbModal(true)}>
            新建数据库
          </button>
        </div>
        {dbs.length === 0 ? (
          <div className="empty-hint">暂无业务数据库</div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>库名</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {dbs.map((db) => (
                <tr key={db}>
                  <td className="mono">{db}</td>
                  <td className="right">
                    <button
                      className="btn small danger-ghost"
                      onClick={() => void dropDb(db)}
                    >
                      删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="panel">
        <div className="panel-head">
          <div>
            <h3>账号</h3>
            <p className="muted small">
              仅授予 SELECT / INSERT / UPDATE / DELETE / CREATE / DROP / ALTER / INDEX。
            </p>
          </div>
          <div className="row-actions">
            <button className="btn small ghost" onClick={() => setRootModal(true)}>
              重置 root 密码
            </button>
            <button className="btn small primary" onClick={() => setUserModal(true)}>
              新建账号
            </button>
          </div>
        </div>
        {users.length === 0 ? (
          <div className="empty-hint">暂无业务账号</div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>账号</th>
                <th>Host</th>
                <th>授权库</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {users.map((row) => (
                <tr key={`${row.user}@${row.host}`}>
                  <td className="mono">{row.user}</td>
                  <td className={row.host === "%" ? "warn-text mono" : "mono"}>
                    {row.host}
                  </td>
                  <td className="muted mono ellipsis">
                    {row.dbs.length ? row.dbs.join(", ") : "-"}
                  </td>
                  <td className="right row-actions">
                    <button
                      className="btn small ghost"
                      onClick={() => {
                        setPwdTarget(row);
                        setNewPassword("");
                      }}
                    >
                      改密码
                    </button>
                    <button
                      className="btn small danger-ghost"
                      onClick={() => void dropUser(row)}
                    >
                      删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {dbModal && (
        <Modal title="新建数据库" onClose={() => setDbModal(false)}>
          <div className="form-grid one-col">
            <label>
              库名
              <input
                value={dbName}
                placeholder="testdb"
                onChange={(e) => setDbName(e.target.value)}
              />
            </label>
            <label>
              字符集
              <input value={charset} onChange={(e) => setCharset(e.target.value)} />
            </label>
            <label>
              排序规则
              <input value={collate} onChange={(e) => setCollate(e.target.value)} />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setDbModal(false)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={isPending("create-db")}
              onClick={() => void createDb()}
            >
              创建
            </button>
          </div>
        </Modal>
      )}

      {userModal && (
        <Modal title="新建账号" onClose={() => setUserModal(false)}>
          <div className="form-grid one-col">
            <label>
              用户名
              <input value={user} onChange={(e) => setUser(e.target.value)} />
            </label>
            <label>
              Host
              <select value={host} onChange={(e) => setHost(e.target.value)}>
                <option value="localhost">localhost（仅本机）</option>
                <option value="127.0.0.1">127.0.0.1</option>
                <option value="%">%（任意来源，存在安全风险）</option>
              </select>
            </label>
            {host === "%" && (
              <div className="inline-warning">
                允许任意来源连接会显著扩大攻击面，仅建议在隔离的开发网络中使用。
              </div>
            )}
            <label>
              密码
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <div>
              <div className="field-label">授权数据库</div>
              <div className="checkbox-list">
                {dbs.length === 0 && <span className="muted small">暂无数据库</span>}
                {dbs.map((db) => (
                  <label key={db} className="check-inline">
                    <input
                      type="checkbox"
                      checked={selectedDbs.includes(db)}
                      onChange={(e) =>
                        setSelectedDbs((prev) =>
                          e.target.checked
                            ? [...prev, db]
                            : prev.filter((d) => d !== db),
                        )
                      }
                    />
                    {db}
                  </label>
                ))}
              </div>
            </div>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setUserModal(false)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={isPending("create-user")}
              onClick={() => void createUser()}
            >
              创建
            </button>
          </div>
        </Modal>
      )}

      {pwdTarget && (
        <Modal
          title={`修改密码 ${pwdTarget.user}@${pwdTarget.host}`}
          onClose={() => setPwdTarget(null)}
        >
          <div className="form-grid one-col">
            <label>
              新密码
              <input
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
              />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setPwdTarget(null)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={isPending("alter-user-password")}
              onClick={() => void alterPassword()}
            >
              保存
            </button>
          </div>
        </Modal>
      )}

      {rootModal && (
        <Modal title="重置 root 密码" onClose={() => setRootModal(false)}>
          <div className="inline-warning danger-box">
            高风险操作：MySQL 会以 --skip-grant-tables 模式短暂重启，期间所有本机程序都能无密码访问。
            请确保没有其他业务正在使用该实例。
          </div>
          <div className="form-grid one-col">
            <label>
              新的 root 密码
              <input
                type="password"
                value={rootPassword}
                onChange={(e) => setRootPassword(e.target.value)}
              />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setRootModal(false)}>
              取消
            </button>
            <button
              className="btn danger"
              disabled={isPending("reset-root")}
              onClick={() => void resetRoot()}
            >
              开始重置
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
