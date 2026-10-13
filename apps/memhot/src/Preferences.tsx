import { useState, type FormEvent } from "react";
import { Api, errorText } from "./api";
import { requestNotifications } from "./native";
import { SelectField } from "./ui/select";
import { ChoiceGroup } from "./ui/choice-group";
import { Notice } from "./components";
import type { Profile } from "./types";

export function Preferences({
  api,
  profile,
  section,
  onSaved,
}: {
  api: Api;
  profile: Profile;
  section: "interests" | "schedule" | "identity";
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState(profile);
  const [interests, setInterests] = useState(profile.interests.join("，"));
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    setNotice("");
    const changes =
      section === "interests"
        ? {
            interests: [
              ...new Set(
                interests
                  .split(/[,，\n]/)
                  .map((s) => s.trim())
                  .filter(Boolean),
              ),
            ],
          }
        : section === "identity"
          ? { name: draft.name.trim() }
          : {
              timezone: draft.timezone,
              digestHour: Number(draft.digestHour),
              researchHour: Number(draft.researchHour ?? 23),
              retentionDays: Number(draft.retentionDays),
            };
    try {
      await api.request("/api/v1/profile", "PUT", changes);
      setNotice("已保存");
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  const hours = Array.from({ length: 24 }, (_, i) => ({
    value: String(i),
    label: `${String(i).padStart(2, "0")}:00`,
  }));
  const selected = interests
    .split(/[,，\n]/)
    .map((s) => s.trim())
    .filter(Boolean);
  const presets = [
    "人工智能",
    "机器人",
    "天文学",
    "物理学",
    "产品设计",
    "商业与创业",
    "科技产业",
    "心理与认知",
    "历史与人文",
    "经济与社会",
    "教育学习",
    "健康与生活",
  ];
  return (
    <form className="settings-page" onSubmit={save}>
      {section === "identity" && (
        <section className="settings-section">
          <label>
            怎么称呼你
            <input
              required
              maxLength={60}
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            />
          </label>
        </section>
      )}
      {section === "interests" && (
        <>
          <section className="settings-section">
            <p className="field-hint">
              选择你想持续了解的领域，也可以自己填写。
            </p>
            <ChoiceGroup
              label="选择关注领域"
              choices={presets.map((topic) => ({ id: topic, label: topic }))}
              value={selected}
              onValueChange={(next) => setInterests(next.join("，"))}
              disabled={busy}
            />
            <label>
              关注领域
              <textarea
                rows={3}
                maxLength={1000}
                value={interests}
                onChange={(e) => setInterests(e.target.value)}
                placeholder="用逗号分隔，最多 20 个"
              />
            </label>
            <p className="field-hint">
              人工智能、机器人、天文学和物理学提供统一日报；更具体的兴趣会单独整理。
            </p>
          </section>
        </>
      )}
      {section === "schedule" && (
        <section className="settings-section">
          <div className="form-grid">
            <label>
              夜间开始研究
              <SelectField
                label="夜间开始研究"
                value={String(draft.researchHour ?? 23)}
                onValueChange={(value) =>
                  setDraft({ ...draft, researchHour: +value })
                }
                options={hours}
                disabled={busy}
              />
            </label>
            <label>
              接收材料
              <SelectField
                label="接收材料"
                value={String(draft.digestHour)}
                onValueChange={(value) =>
                  setDraft({ ...draft, digestHour: +value })
                }
                options={hours}
                disabled={busy}
              />
            </label>
          </div>
          <label>
            时区
            <SelectField
              label="时区"
              value={draft.timezone}
              onValueChange={(value) => setDraft({ ...draft, timezone: value })}
              options={[
                "Asia/Shanghai",
                "Asia/Tokyo",
                "Asia/Singapore",
                "Europe/London",
                "America/New_York",
                "America/Los_Angeles",
                "UTC",
              ].map((value) => ({ value, label: value }))}
              disabled={busy}
            />
          </label>
          <button
            className="secondary"
            type="button"
            onClick={() =>
              void requestNotifications()
                .then(() => setNotice("请按系统提示开启通知。"))
                .catch((e) => setError(errorText(e)))
            }
          >
            开启日报提醒
          </button>
          <p className="field-hint">错过推送时，可以在「阅读」同步内容。</p>
          <label>
            原始文字保留
            <SelectField
              label="原始文字保留"
              value={String(draft.retentionDays)}
              onValueChange={(value) =>
                setDraft({ ...draft, retentionDays: +value })
              }
              options={[7, 30, 90, 365].map((value) => ({
                value: String(value),
                label: `${value} 天`,
              }))}
              disabled={busy}
            />
          </label>
          <p className="field-hint">
            来源到期后，依赖它的自动总结也会清理。你手动填写的偏好单独保留。
          </p>
        </section>
      )}
      {error && <Notice error={error} />}
      <div className="settings-save">
        <button className="primary" disabled={busy}>
          {busy ? "保存中…" : "保存"}
        </button>
        {notice && (
          <p role="status" className="success-message">
            {notice}
          </p>
        )}
      </div>
    </form>
  );
}
