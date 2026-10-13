import { Api } from "./api";
import { Account } from "./Account";
import { Preferences } from "./Preferences";
import { DataControls } from "./DataControls";
import { PreferencePortrait } from "./PreferencePortrait";
import { Cognition } from "./Cognition";
import type { Profile } from "./types";
import { Icon } from "./Icon";
import { Updates } from "./AppUpdates";
import { Subscriptions } from "./Subscriptions";

export type PreferenceSection =
  | "overview"
  | "interests"
  | "subscriptions"
  | "cognition-list"
  | "cognition"
  | "schedule"
  | "account"
  | "privacy"
  | "updates";
export function PersonalPreferences({
  api,
  profile,
  refreshKey,
  section,
  onSection,
  onSaved,
  onDisconnect,
  onCleared,
  onSource,
}: {
  api: Api;
  profile: Profile;
  refreshKey: number;
  section: PreferenceSection;
  onSection: (section: PreferenceSection) => void;
  onSaved: () => void;
  onDisconnect: () => void;
  onCleared: () => void;
  onSource: (id: string) => void;
}) {
  const rows: { id: PreferenceSection; title: string; detail: string }[] = [
    {
      id: "interests",
      title: "关注领域",
      detail: profile.interests.length
        ? profile.interests.join("、")
        : "选择想持续了解的领域",
    },
    { id: "subscriptions", title: "我的订阅", detail: "添加常读的来源，导入订阅，查看更新" },
    {
      id: "cognition",
      title: "对我的了解",
      detail: "阅读偏好、目标与判断，随时修正",
    },
    {
      id: "schedule",
      title: "推送与保存",
      detail: `每天 ${String(profile.digestHour).padStart(2, "0")}:00 接收 · 文字保留 ${profile.retentionDays} 天`,
    },
    {
      id: "account",
      title: "账号与额度",
      detail: "登录账户、测试套餐和使用额度",
    },
    {
      id: "privacy",
      title: "数据与隐私",
      detail: "存储说明、清理数据和退出登录",
    },
    { id: "updates", title: "应用更新", detail: "检查新版本，查看更新内容" },
  ];
  return (
    <section className="personal-preferences">
      {section !== "overview" && (
        <button
          className="text-button back-button"
          onClick={() => onSection(section === "cognition-list" ? "cognition" : "overview")}
        >
          <Icon name="back" size={18} />
          {section === "cognition-list" ? "对我的了解" : "个人偏好"}
        </button>
      )}
      {section === "overview" ? (
        <>
          <div className="page-heading">
            <div>
              <h1>个人偏好</h1>
              <p className="muted">兴趣、阅读习惯和账户，都在这里。</p>
            </div>
          </div>
          <button
            className="preference-identity"
            onClick={() => onSection("account")}
            aria-label="账号与额度"
          >
            <span className="avatar">{(profile.name || "我").slice(0, 1)}</span>
            <span>
              {profile.name || "我的空间"}
              <small>
                账号与额度 <span aria-hidden="true">·</span> 查看套餐和使用情况
              </small>
            </span>
            <Icon name="chevron" size={18} />
          </button>
          <div className="preference-menu">
            {rows
              .filter((row) => row.id !== "account")
              .map((row) => (
                <button key={row.id} onClick={() => onSection(row.id)}>
                  <span>
                    <strong>{row.title}</strong>
                    <small>{row.detail}</small>
                  </span>
                  <Icon name="chevron" size={18} />
                </button>
              ))}
          </div>
        </>
      ) : section === "cognition" ? (
        <PreferencePortrait
          api={api}
          active
          refreshKey={refreshKey}
          fixedInterests={profile.interests}
          onPreferences={() => onSection("interests")}
          onAll={() => onSection("cognition-list")}
          onSource={onSource}
        />
      ) : section === "cognition-list" ? (
        <Cognition api={api} onSource={onSource} />
      ) : (
        <>
          <div className="page-heading">
            <h1>{rows.find((row) => row.id === section)?.title}</h1>
          </div>
          {section === "interests" && (
            <Preferences
              key="interests"
              section="interests"
              api={api}
              profile={profile}
              onSaved={onSaved}
            />
          )}
          {section === "schedule" && (
            <Preferences
              key="schedule"
              section="schedule"
              api={api}
              profile={profile}
              onSaved={onSaved}
            />
          )}
          {section === "subscriptions" && <Subscriptions api={api} />}
          {section === "account" && (
            <>
              <Account api={api} />
              <Preferences
                key="identity"
                section="identity"
                api={api}
                profile={profile}
                onSaved={onSaved}
              />
            </>
          )}
          {section === "privacy" && (
            <DataControls
              api={api}
              onCleared={onCleared}
              onDisconnect={onDisconnect}
              onSaved={onSaved}
            />
          )}
          {section === "updates" && <Updates />}
        </>
      )}
    </section>
  );
}
