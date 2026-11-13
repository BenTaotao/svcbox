import { useCallback, useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";
import type { CertificateInfo } from "../types";

export function CertManager() {
  const { toast, runAction, isPending } = useApp();
  const [certs, setCerts] = useState<CertificateInfo[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    try {
      setCerts(await call<CertificateInfo[]>("list_certificates"));
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const renew = async (cert: CertificateInfo) => {
    const result = await runAction(
      `renew-${cert.instanceId}-${cert.domain}`,
      () =>
        call<CertificateInfo>("renew_certificate", {
          instanceId: cert.instanceId,
          domain: cert.domain,
        }),
      `证书 ${cert.domain} 已续期`,
    );
    if (result) await load();
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>证书管理</h1>
          <p className="muted">
            后台每天检查一次，剩余不足 30 天的证书自动续期（实例需处于运行状态）。
          </p>
        </div>
        <button className="btn ghost" onClick={() => void load()}>
          刷新
        </button>
      </header>

      <section className="panel no-pad">
        {loading ? (
          <div className="empty-hint">加载中...</div>
        ) : certs.length === 0 ? (
          <div className="empty-panel borderless">
            <div className="empty-title">还没有证书</div>
            <p className="muted">
              在 Nginx 实例的站点页签中，为监听 80 端口的站点申请 HTTPS 证书。
            </p>
          </div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th>域名</th>
                <th>实例</th>
                <th>签发时间</th>
                <th>到期时间</th>
                <th>剩余</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {certs.map((cert) => {
                const urgent = cert.daysLeft < 30;
                return (
                  <tr key={`${cert.instanceId}-${cert.domain}`}>
                    <td className="mono">{cert.domain}</td>
                    <td className="mono muted">{cert.instanceName}</td>
                    <td className="mono muted">{cert.issuedAt.slice(0, 19)}</td>
                    <td className="mono">{cert.expiresAt.slice(0, 19)}</td>
                    <td>
                      <span
                        className={`tag ${urgent ? "tag-warn" : "tag-ok"}`}
                        title={cert.certPath}
                      >
                        {cert.daysLeft} 天
                      </span>
                    </td>
                    <td className="right">
                      <button
                        className="btn small ghost"
                        disabled={isPending(`renew-${cert.instanceId}-${cert.domain}`)}
                        onClick={() => void renew(cert)}
                      >
                        手动续期
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
