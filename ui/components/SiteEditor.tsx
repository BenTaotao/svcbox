import { useCallback, useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";
import type { CertificateInfo, SiteConfig } from "../types";
import { Modal } from "./Modal";
import { NginxGlobalForm } from "./NginxGlobalForm";

const EMPTY_SITE: SiteConfig = {
  serverName: "",
  root: "",
  index: "index.html index.htm",
  customLocations: "",
  https: false,
  certDomain: null,
};

export function SiteEditor({ instanceId }: { instanceId: string }) {
  const { toast, confirm, runAction, isPending } = useApp();
  const [sites, setSites] = useState<SiteConfig[]>([]);
  const [loading, setLoading] = useState(true);
  const [editor, setEditor] = useState<SiteConfig | null>(null);
  const [isNew, setIsNew] = useState(true);
  const [httpsTarget, setHttpsTarget] = useState<SiteConfig | null>(null);
  const [domain, setDomain] = useState("");

  const load = useCallback(async () => {
    try {
      setSites(await call<SiteConfig[]>("list_nginx_sites", { instanceId }));
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setLoading(false);
    }
  }, [instanceId, toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    if (!editor) return;
    const result = await runAction(
      "save-site",
      () =>
        call<SiteConfig>(isNew ? "create_nginx_site" : "update_nginx_site", {
          instanceId,
          site: editor,
        }),
      isNew ? "站点已创建" : "站点已更新",
    );
    if (result !== undefined) {
      setEditor(null);
      await load();
    }
  };

  const remove = async (site: SiteConfig) => {
    const ok = await confirm({
      title: `删除站点 ${site.serverName}`,
      message: `只会删除 Nginx 站点配置，不会删除网站根目录：\n${site.root}\n\n该操作不可撤销。`,
      danger: true,
      confirmText: "删除站点",
    });
    if (!ok) return;
    const result = await runAction("delete-site", () =>
      call<void>("delete_nginx_site", {
        instanceId,
        serverName: site.serverName,
      }),
    );
    if (result !== undefined) {
      toast("站点已删除，网站目录保持不变", "success");
      await load();
    }
  };

  const applyHttps = async () => {
    if (!httpsTarget) return;
    const result = await runAction("apply-https", () =>
      call<CertificateInfo>("apply_site_https", {
        instanceId,
        serverName: httpsTarget.serverName,
        domain,
      }),
    );
    if (result) {
      toast(`证书已签发，到期时间 ${result.expiresAt}`, "success");
      setHttpsTarget(null);
      await load();
    }
  };

  const renew = async (site: SiteConfig) => {
    const certDomain = site.certDomain ?? site.serverName;
    const result = await runAction(
      `renew-${site.serverName}`,
      () =>
        call<CertificateInfo>("renew_certificate", {
          instanceId,
          domain: certDomain,
        }),
      `证书 ${certDomain} 已续期`,
    );
    if (result) await load();
  };

  const reload = async () => {
    await runAction(
      "reload-nginx",
      () => call<void>("reload_nginx", { instanceId }),
      "Nginx 已 reload",
    );
  };

  return (
    <div className="stack">
      <section className="panel">
        <div className="panel-head">
          <div>
            <h3>站点管理</h3>
            <p className="muted small">
              域名解析到本机且 80 端口放行后，可直接申请 Let&apos;s Encrypt 证书。
            </p>
          </div>
          <div className="row-actions">
            <button className="btn small ghost" onClick={() => void reload()}>
              重载 Nginx
            </button>
            <button
              className="btn small primary"
              onClick={() => {
                setEditor({ ...EMPTY_SITE });
                setIsNew(true);
              }}
            >
              新增站点
            </button>
          </div>
        </div>

        {loading ? (
          <div className="muted">加载中...</div>
        ) : sites.length === 0 ? (
          <div className="empty-hint">还没有站点，先创建一个静态站点。</div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>域名</th>
                <th>根目录</th>
                <th>协议</th>
                <th>证书域名</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {sites.map((site) => (
                <tr key={site.serverName}>
                  <td className="mono">{site.serverName}</td>
                  <td className="mono muted ellipsis">{site.root}</td>
                  <td>
                    <span className={`tag ${site.https ? "tag-ok" : ""}`}>
                      {site.https ? "HTTPS" : "HTTP"}
                    </span>
                  </td>
                  <td className="mono muted">{site.certDomain ?? "-"}</td>
                  <td className="right row-actions">
                    <button
                      className="btn small ghost"
                      onClick={() => {
                        setEditor({ ...site });
                        setIsNew(false);
                      }}
                    >
                      编辑
                    </button>
                    <button
                      className="btn small ghost"
                      disabled={isPending(`renew-${site.serverName}`)}
                      onClick={() =>
                        site.https
                          ? void renew(site)
                          : (setHttpsTarget(site), setDomain(site.serverName))
                      }
                    >
                      {site.https ? "续期" : "申请 HTTPS"}
                    </button>
                    <button className="btn small danger-ghost" onClick={() => void remove(site)}>
                      删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <NginxGlobalForm instanceId={instanceId} />

      {editor && (
        <Modal
          title={isNew ? "新增站点" : `编辑站点 ${editor.serverName}`}
          onClose={() => setEditor(null)}
        >
          <div className="form-grid one-col">
            <label>
              域名 server_name
              <input
                value={editor.serverName}
                placeholder="test.com"
                onChange={(e) => setEditor({ ...editor, serverName: e.target.value })}
              />
            </label>
            <label>
              网站根目录
              <input
                value={editor.root}
                placeholder="D:/www/test"
                onChange={(e) => setEditor({ ...editor, root: e.target.value })}
              />
            </label>
            <label>
              index
              <input
                value={editor.index}
                onChange={(e) => setEditor({ ...editor, index: e.target.value })}
              />
            </label>
            <label>
              自定义 location 片段
              <textarea
                className="code-area small-area"
                value={editor.customLocations}
                placeholder={'location /api/ {\n    proxy_pass http://127.0.0.1:3000;\n}'}
                onChange={(e) =>
                  setEditor({ ...editor, customLocations: e.target.value })
                }
              />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setEditor(null)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={isPending("save-site")}
              onClick={() => void save()}
            >
              {isNew ? "创建" : "保存"}
            </button>
          </div>
        </Modal>
      )}

      {httpsTarget && (
        <Modal
          title={`为 ${httpsTarget.serverName} 申请证书`}
          onClose={() => setHttpsTarget(null)}
        >
          <div className="inline-warning">
            前提：域名已解析到本机，实例监听 80 端口，且防火墙放行 80。HTTP-01 验证失败会直接展示
            acme.sh 原始错误。
          </div>
          <div className="form-grid one-col">
            <label>
              证书域名
              <input value={domain} onChange={(e) => setDomain(e.target.value)} />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn ghost" onClick={() => setHttpsTarget(null)}>
              取消
            </button>
            <button
              className="btn primary"
              disabled={isPending("apply-https")}
              onClick={() => void applyHttps()}
            >
              开始申请
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
