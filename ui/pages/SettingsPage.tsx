import { useEffect, useState } from "react";
import { call } from "../api";
import { DoctorModal } from "../components/DoctorModal";
import { useApp } from "../store/appStore";
import type { DownloadSource, Settings, SettingsResult } from "../types";

export function SettingsPage() {
  const { settings, paths, reloadSettings, runAction, toast, isPending } = useApp();
  const [draft, setDraft] = useState<Settings | null>(null);
  const [restartRequired, setRestartRequired] = useState(false);
  const [doctorOpen, setDoctorOpen] = useState(false);

  useEffect(() => {
    if (settings) setDraft({ ...settings });
  }, [settings]);

  if (!draft) return <div className="page muted">加载设置中...</div>;

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setDraft((prev) => (prev ? { ...prev, [key]: value } : prev));

  const save = async () => {
    const result = await runAction(
      "save-settings",
      () => call<SettingsResult>("update_settings", { settings: draft }),
      "设置已保存",
    );
    if (result) {
      setDraft({ ...result.settings });
      setRestartRequired(result.restartRequired);
      if (result.restartRequired) {
        toast("部分设置需要重启 svcbox 后生效", "info");
      }
      await reloadSettings();
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>设置</h1>
          <p className="muted">数据目录、版本清单源、启动行为和本地 HTTP API。</p>
        </div>
        <div className="row-actions">
          <button className="btn ghost" onClick={() => setDoctorOpen(true)}>
            环境自检
          </button>
          <button
            className="btn primary"
            disabled={isPending("save-settings")}
            onClick={() => void save()}
          >
            保存设置
          </button>
        </div>
      </header>

      {restartRequired && (
        <div className="inline-warning">
          数据目录、HTTP API 开关/端口、日志级别的修改需要重启 svcbox 后生效。
        </div>
      )}

      <section className="panel">
        <h3>数据与版本源</h3>
        <div className="form-grid">
          <label className="span-2">
            数据目录（版本、实例、下载缓存、数据库、证书都放在这里；默认 D:\BC\svcbox，留空恢复默认）
            <input
              value={draft.dataDir ?? ""}
              placeholder={paths?.home ?? "D:\\BC\\svcbox"}
              onChange={(e) => update("dataDir", e.target.value || null)}
            />
          </label>
          <label className="span-2">
            远端版本清单地址（HTTPS，可选）
            <input
              value={draft.manifestUrl ?? ""}
              placeholder="https://example.com/svcbox-manifest.json"
              onChange={(e) => update("manifestUrl", e.target.value || null)}
            />
          </label>
          <label>
            下载源
            <select
              value={draft.downloadSource}
              onChange={(e) => update("downloadSource", e.target.value as DownloadSource)}
            >
              <option value="official">官方源（默认）</option>
              <option value="mirror">国内镜像备用源（未配置时回退官方源）</option>
            </select>
          </label>
          <label>
            日志级别
            <select
              value={draft.logLevel}
              onChange={(e) => update("logLevel", e.target.value)}
            >
              <option value="trace">trace</option>
              <option value="debug">debug</option>
              <option value="info">info</option>
              <option value="warn">warn</option>
              <option value="error">error</option>
            </select>
          </label>
        </div>
        <p className="muted small">
          修改数据目录后需重启生效；已有版本、实例、数据库和证书不会自动搬迁，请先退出程序再手动移动旧目录内容。也可以启动时加
          --home 或 --data-dir 指定目录。
        </p>
      </section>

      <section className="panel">
        <h3>行为开关</h3>
        <div className="form-grid">
          <label className="check-row">
            <input
              type="checkbox"
              checked={draft.stopInstancesOnClose}
              onChange={(e) => update("stopInstancesOnClose", e.target.checked)}
            />
            关闭窗口时停止所有实例（默认：后台常驻，不停止）
          </label>
          <label className="check-row">
            <input
              type="checkbox"
              checked={draft.autostart}
              onChange={(e) => update("autostart", e.target.checked)}
            />
            开机自启（静默启动到托盘）
          </label>
          <label className="check-row">
            <input
              type="checkbox"
              checked={draft.httpApiEnabled}
              onChange={(e) => update("httpApiEnabled", e.target.checked)}
            />
            开启本地 HTTP API
          </label>
          <label>
            HTTP API 端口
            <input
              type="number"
              min={1}
              max={65535}
              value={draft.httpApiPort}
              disabled={!draft.httpApiEnabled}
              onChange={(e) => update("httpApiPort", Number(e.target.value))}
            />
          </label>
        </div>
        <p className="muted small">
          HTTP API 只监听 127.0.0.1:端口，无鉴权，仅限本机程序调用；服务端校验 Host/Origin，拒绝 DNS rebinding 与浏览器跨站请求。默认关闭。
        </p>
      </section>

      <section className="panel">
        <h3>运行目录</h3>
        <div className="info-grid">
          <div>
            <span className="info-label">数据目录</span>
            <span className="mono selectable">{paths?.home}</span>
          </div>
          <div>
            <span className="info-label">SQLite</span>
            <span className="mono selectable">{paths?.dbFile}</span>
          </div>
          <div>
            <span className="info-label">版本</span>
            <span className="mono selectable">{paths?.versions}</span>
          </div>
          <div>
            <span className="info-label">实例</span>
            <span className="mono selectable">{paths?.instances}</span>
          </div>
          <div>
            <span className="info-label">缓存</span>
            <span className="mono selectable">{paths?.cache}</span>
          </div>
          <div>
            <span className="info-label">日志</span>
            <span className="mono selectable">{paths?.logs}</span>
          </div>
        </div>
      </section>

      {doctorOpen && <DoctorModal onClose={() => setDoctorOpen(false)} />}
    </div>
  );
}
