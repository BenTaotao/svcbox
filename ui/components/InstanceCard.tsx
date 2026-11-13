import { call } from "../api";
import { useApp } from "../store/appStore";
import type { InstanceView } from "../types";
import { HealthBadge } from "./HealthBadge";
import { KindBadge } from "./KindBadge";

export function InstanceCard({ instance }: { instance: InstanceView }) {
  const { openInstance, runAction, refreshInstances, isPending } = useApp();
  const active = ["running", "warningPortNotListen", "errorLogHasError"].includes(
    instance.health,
  );

  const act = async (command: "start" | "stop" | "restart") => {
    const result = await runAction(`${command}-${instance.id}`, () =>
      call<InstanceView>(`${command}_instance`, { id: instance.id }),
    );
    if (result) await refreshInstances(true);
  };

  return (
    <article
      className={`instance-card health-border-${instance.health}`}
      onDoubleClick={() => openInstance(instance.id)}
    >
      <header className="instance-card-head">
        <KindBadge kind={instance.kind} withName />
        <HealthBadge status={instance.health} />
      </header>
      <div className="instance-name">{instance.name}</div>
      <div className="instance-meta">
        <span className="mono">v{instance.version}</span>
        <span className="meta-sep">·</span>
        <span className="mono">:{instance.port}</span>
        {instance.pid ? (
          <>
            <span className="meta-sep">·</span>
            <span className="mono muted">pid {instance.pid}</span>
          </>
        ) : null}
      </div>
      <div className="instance-url mono">{instance.urls[0]}</div>
      <footer className="instance-card-foot">
        <button className="btn small ghost" onClick={() => openInstance(instance.id)}>
          详情
        </button>
        <span className="spacer" />
        {active ? (
          <>
            <button
              className="btn small ghost"
              disabled={isPending(`stop-${instance.id}`)}
              onClick={() => void act("stop")}
            >
              停止
            </button>
            <button
              className="btn small"
              disabled={isPending(`restart-${instance.id}`)}
              onClick={() => void act("restart")}
            >
              重启
            </button>
          </>
        ) : (
          <button
            className="btn small primary"
            disabled={isPending(`start-${instance.id}`)}
            onClick={() => void act("start")}
          >
            启动
          </button>
        )}
      </footer>
    </article>
  );
}
