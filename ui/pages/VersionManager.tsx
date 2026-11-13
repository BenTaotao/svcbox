import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { call } from "../api";
import { DownloadProgressModal } from "../components/DownloadProgressModal";
import { KindBadge, kindName } from "../components/KindBadge";
import { useApp } from "../store/appStore";
import type { DownloadProgress, VersionInfo } from "../types";

function keyOf(v: VersionInfo): string {
  return `${v.kind}-${v.version}`;
}

export function VersionManager() {
  const {
    versions,
    settings,
    refreshVersions,
    runAction,
    confirm,
    toast,
    isPending,
    navigate,
  } = useApp();
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);

  useEffect(() => {
    void refreshVersions();
  }, [refreshVersions]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<DownloadProgress>("download-progress", (event) => {
      setProgress(event.payload);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  const install = async (v: VersionInfo) => {
    const key = keyOf(v);
    setInstalling(key);
    setProgress({
      kind: v.kind,
      version: v.version,
      downloaded: 0,
      total: null,
      percent: 0,
      stage: "准备下载",
    });
    const result = await runAction(`install-${key}`, () =>
      call<VersionInfo>("install_version", { kind: v.kind, version: v.version }),
    );
    setInstalling(null);
    if (result) {
      toast(`${kindName(v.kind)} ${v.version} 安装完成`, "success");
      await refreshVersions();
      window.setTimeout(() => setProgress(null), 1500);
    } else {
      setProgress(null);
    }
  };

  const remove = async (v: VersionInfo) => {
    const ok = await confirm({
      title: `删除 ${kindName(v.kind)} ${v.version}`,
      message: "已解压的版本目录会被删除。若仍有实例引用该版本，操作会被拒绝。",
      danger: true,
      confirmText: "删除版本",
    });
    if (!ok) return;
    const result = await runAction(`remove-${keyOf(v)}`, () =>
      call<void>("remove_version", { kind: v.kind, version: v.version }),
    );
    if (result !== undefined) {
      toast("版本已删除", "success");
      await refreshVersions();
    }
  };

  const pruneVersions = async () => {
    const ok = await confirm({
      title: "清理旧版本",
      message:
        "将删除不在当前清单、且没有实例引用的旧版本目录。仍被实例使用的版本会自动保留。",
      danger: true,
      confirmText: "开始清理",
    });
    if (!ok) return;
    const removed = await runAction("prune-versions", () =>
      call<string[]>("prune_versions"),
    );
    if (removed) {
      toast(
        removed.length ? `已清理 ${removed.length} 个旧版本` : "没有可清理的旧版本",
        "success",
      );
      await refreshVersions();
    }
  };

  const refreshManifest = async () => {
    const url = settings?.manifestUrl;
    if (!url) {
      toast("请先在设置页填写远端清单地址（HTTPS）", "error");
      return;
    }
    const count = await runAction(
      "refresh-manifest",
      () => call<number>("refresh_manifest", { url }),
      "远端版本清单已更新",
    );
    if (count !== undefined) await refreshVersions();
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>版本仓库</h1>
          <p className="muted">
            官方源下载 + sha256 强校验，原始压缩包保留在缓存目录复用。
          </p>
        </div>
        <div className="row-actions">
          <span className="tag">
            下载源：{settings?.downloadSource === "mirror" ? "国内镜像" : "官方源"}
          </span>
          <button className="btn ghost" onClick={() => void refreshManifest()}>
            刷新远端清单
          </button>
          <button
            className="btn ghost"
            disabled={isPending("prune-versions")}
            onClick={() => void pruneVersions()}
          >
            清理旧版本
          </button>
        </div>
      </header>

      <section className="panel no-pad">
        <table className="data-table">
          <thead>
            <tr>
              <th>类型</th>
              <th>版本</th>
              <th>平台</th>
              <th>状态</th>
              <th>sha256</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {versions.map((v) => (
              <tr key={keyOf(v)}>
                <td>
                  <KindBadge kind={v.kind} withName />
                </td>
                <td className="mono">{v.version}</td>
                <td className="mono muted">{v.platform}</td>
                <td>
                  <span className={`tag ${v.installed ? "tag-ok" : ""}`}>
                    {v.installed ? "已安装" : "可下载"}
                  </span>
                </td>
                <td className="mono muted ellipsis" title={v.sha256}>
                  {v.sha256 ? `${v.sha256.slice(0, 12)}…` : "未配置"}
                </td>
                <td className="right row-actions">
                  {v.installed ? (
                    <>
                      <button
                        className="btn small ghost"
                        onClick={() => navigate("dashboard")}
                      >
                        去创建实例
                      </button>
                      <button
                        className="btn small danger-ghost"
                        disabled={isPending(`remove-${keyOf(v)}`)}
                        onClick={() => void remove(v)}
                      >
                        删除
                      </button>
                    </>
                  ) : (
                    <button
                      className="btn small primary"
                      disabled={installing !== null || !v.sha256}
                      title={!v.sha256 ? "清单缺少 sha256，拒绝安装" : ""}
                      onClick={() => void install(v)}
                    >
                      {installing === keyOf(v) ? "安装中..." : "下载安装"}
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {versions.length === 0 && (
              <tr>
                <td colSpan={6} className="muted center">
                  暂无版本数据
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>

      <p className="muted small">
        安装包地址来自内置或远端 manifest，程序不托管安装包。若官方地址不可达，可下载后使用
        `svcbox cli version import` 手动导入并校验 sha256。
      </p>

      {progress && (
        <DownloadProgressModal
          progress={progress}
          onClose={() => setProgress(null)}
        />
      )}
    </div>
  );
}
