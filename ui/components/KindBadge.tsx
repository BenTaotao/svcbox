import type { ServiceKind } from "../types";

const META: Record<ServiceKind, { label: string; name: string }> = {
  nginx: { label: "N", name: "Nginx" },
  mysql: { label: "MY", name: "MySQL" },
  redis: { label: "R", name: "Redis" },
};

export function KindBadge({
  kind,
  withName,
}: {
  kind: ServiceKind;
  withName?: boolean;
}) {
  const meta = META[kind];
  return (
    <span className={`kind-badge kind-${kind}`}>
      <span className="kind-mark">{meta.label}</span>
      {withName && <span className="kind-name">{meta.name}</span>}
    </span>
  );
}

export function kindName(kind: ServiceKind): string {
  return META[kind].name;
}
