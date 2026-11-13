import { CertManager } from "./pages/CertManager";
import { Dashboard } from "./pages/Dashboard";
import { InstanceDetail } from "./pages/InstanceDetail";
import { SettingsPage } from "./pages/SettingsPage";
import { VersionManager } from "./pages/VersionManager";
import { useApp, type Page } from "./store/appStore";

const NAV: Array<{ id: Page; label: string; glyph: string }> = [
  { id: "dashboard", label: "仪表盘", glyph: "▦" },
  { id: "versions", label: "版本仓库", glyph: "⬇" },
  { id: "certs", label: "证书管理", glyph: "✳" },
  { id: "settings", label: "设置", glyph: "⚙" },
];

export default function App() {
  const { page, navigate, instances } = useApp();
  const running = instances.filter((i) =>
    ["running", "warningPortNotListen", "errorLogHasError"].includes(i.health),
  ).length;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">sv</div>
          <div>
            <div className="brand-name">svcbox</div>
            <div className="brand-sub">本地服务管理器</div>
          </div>
        </div>

        <nav className="nav">
          {NAV.map((item) => (
            <button
              key={item.id}
              className={`nav-item ${page === item.id ? "active" : ""}`}
              onClick={() => navigate(item.id)}
            >
              <span className="nav-glyph">{item.glyph}</span>
              {item.label}
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <div className="mini-stat">
            <span className="mini-stat-value">{running}</span>
            <span className="muted small">个实例运行中</span>
          </div>
          <div className="muted small">Tauri 2 · Rust · React 19</div>
        </div>
      </aside>

      <main className="main">
        {page === "dashboard" && <Dashboard />}
        {page === "versions" && <VersionManager />}
        {page === "certs" && <CertManager />}
        {page === "settings" && <SettingsPage />}
        {page === "instance" && <InstanceDetail />}
      </main>
    </div>
  );
}
