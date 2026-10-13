import type { ReactNode } from "react";
import { Icon, Mark, type IconName } from "./Icon";
import type { CaptureStatus } from "./types";

export type Tab = "today" | "thoughts" | "records" | "preferences";
const tabs: { id: Tab; icon: IconName; label: string; description: string }[] =
  [
    {
      id: "records",
      icon: "mic",
      label: "录音",
      description: "录音与导入",
    },
    {
      id: "thoughts",
      icon: "thought",
      label: "思考",
      description: "话题与问题",
    },
    { id: "today", icon: "book", label: "阅读", description: "日报与深度分析" },
    {
      id: "preferences",
      icon: "person",
      label: "个人偏好",
      description: "兴趣、偏好与账户",
    },
  ];

export function AppFrame({
  tab,
  name,
  loading,
  capture,
  focused,
  onNavigate,
  onCapture,
  onRefresh,
  children,
}: {
  tab: Tab;
  name: string;
  loading: boolean;
  capture: CaptureStatus;
  focused: boolean;
  onNavigate: (tab: Tab) => void;
  onCapture: () => void;
  onRefresh: () => void;
  children: ReactNode;
}) {
  const recording = capture.state === "recording";
  return (
    <div className={`app-shell ${focused ? "focused" : ""}`}>
      <aside className="sidebar">
        <button
          className="wordmark brand-button"
          onClick={() => onNavigate("records")}
          aria-label="Memhot首页"
        >
          <Mark />
          <span>Memhot</span>
        </button>
        <nav aria-label="主导航">
          {tabs.map((item) => (
            <button
              key={item.id}
              className={`nav-item ${tab === item.id ? "selected" : ""}`}
              onClick={() => onNavigate(item.id)}
              aria-current={tab === item.id ? "page" : undefined}
            >
              <Icon name={item.icon} />
              <span>
                {item.label}
                <small>{item.description}</small>
              </span>
            </button>
          ))}
        </nav>
        <div className="sidebar-note">
          从一句随想，
          <br />
          到更完整的思考。
        </div>
        <div className="sidebar-profile">
          <span className="avatar">{(name || "我").slice(0, 1)}</span>
          <span>{name || "我的空间"}</span>
        </div>
      </aside>
      <div className="main-shell">
        <header className="topbar">
          <button
            className="mobile-brand"
            onClick={() => onNavigate("records")}
            aria-label="Memhot首页"
          >
            Memhot<span className="brand-period">.</span>
          </button>
          <span className="desktop-page-name">
            {tabs.find((t) => t.id === tab)?.description}
          </span>
          <div className="top-actions">
            {(recording || capture.error || capture.queuedCount > 0) && (
              <button
                className={`capture-chip ${recording ? "recording" : ""}`}
                onClick={onCapture}
              >
                <span className="small-dot" />
                <span>
                  {recording
                    ? "录音中"
                    : capture.error
                      ? "录音待处理"
                      : `${capture.queuedCount} 段待同步`}
                </span>
              </button>
            )}
            <button
              className="icon-button refresh-button"
              aria-label="刷新内容"
              onClick={onRefresh}
              disabled={loading}
            >
              <Icon name="refresh" size={18} />
            </button>
          </div>
        </header>
        <main id="main" className={`workspace ${focused ? "reading" : ""}`}>
          {children}
        </main>
        {!focused && (
          <nav className="bottom-nav" aria-label="移动端导航">
            {tabs.map((item) => (
              <button
                key={item.id}
                className={tab === item.id ? "selected" : ""}
                onClick={() => onNavigate(item.id)}
                aria-current={tab === item.id ? "page" : undefined}
              >
                <span className="nav-icon">
                  <Icon name={item.icon} size={21} />
                </span>
                <span>{item.label}</span>
              </button>
            ))}
          </nav>
        )}
      </div>
    </div>
  );
}
