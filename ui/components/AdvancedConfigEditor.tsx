import { useEffect, useState } from "react";
import { call } from "../api";
import { useApp } from "../store/appStore";

export function AdvancedConfigEditor({ instanceId }: { instanceId: string }) {
  const { toast, confirm, runAction, isPending } = useApp();
  const [content, setContent] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    call<string>("read_raw_config", { id: instanceId })
      .then((text) => {
        if (!disposed) {
          setContent(text);
          setLoaded(true);
        }
      })
      .catch((e: unknown) => {
        if (!disposed) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      disposed = true;
    };
  }, [instanceId]);

  const save = async () => {
    const ok = await confirm({
      title: "保存原始配置",
      message:
        "直接编辑原始配置存在风险：保存前会自动备份并执行语法校验，校验失败会回滚。确认继续？",
      danger: true,
      confirmText: "校验并保存",
    });
    if (!ok) return;
    const result = await runAction(
      `raw-config-${instanceId}`,
      () => call<void>("save_raw_config", { id: instanceId, content }),
      "配置已保存",
    );
    if (result !== undefined) toast("已触发配置重载（若实例正在运行）", "info");
  };

  if (error) return <div className="inline-error">{error}</div>;
  if (!loaded) return <div className="muted">加载配置中...</div>;

  return (
    <div className="raw-editor">
      <div className="inline-warning">
        高级入口：这里的修改会绕过表单。保存时自动备份，并执行 nginx -t / Redis 结构校验。
      </div>
      <textarea
        className="code-area"
        spellCheck={false}
        value={content}
        onChange={(e) => setContent(e.target.value)}
      />
      <div className="row-end">
        <button
          className="btn primary"
          disabled={isPending(`raw-config-${instanceId}`)}
          onClick={() => void save()}
        >
          保存配置
        </button>
      </div>
    </div>
  );
}
