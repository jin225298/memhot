import { useEffect, useState } from "react";
import { Api, errorText } from "./api";
import { Icon, Mark } from "./Icon";
import { ChoiceGroup } from "./ui/choice-group";
import { Loading, Notice } from "./components";
import { useBackAction } from "./useBackAction";

type Choice = { id: string; label: string; statement?: string };
interface SetupCatalog {
  completed: boolean;
  interests: Choice[];
  reading: Choice[];
  thinking: Choice[];
}
export function PreferenceSetup({
  api,
  onDone,
}: {
  api: Api;
  onDone: () => void;
}) {
  const [catalog, setCatalog] = useState<SetupCatalog | null>(null);
  const [step, setStep] = useState(0);
  const [interests, setInterests] = useState<string[]>([]);
  const [reading, setReading] = useState<string[]>([]);
  const [thinking, setThinking] = useState<string[]>([]);
  const [custom, setCustom] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  useBackAction(step > 0, () => { if (!busy) setStep(0); });
  useEffect(() => {
    const controller = new AbortController();
    void api
      .request<SetupCatalog>(
        "/api/v1/preferences/setup",
        "GET",
        undefined,
        controller.signal,
      )
      .then((data) => {
        if (!controller.signal.aborted) {
          if (data.completed) onDone();
          else {
            setCatalog(data);
            setError("");
          }
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      });
    return () => controller.abort();
    // onDone only advances the parent; loading is tied to this authenticated API instance.
  }, [api, reload]);
  async function save(skip = false) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await api.request(
        "/api/v1/preferences/setup",
        "POST",
        skip
          ? { skip: true }
          : {
              interestIds: interests,
              customInterests: custom
                .split(/[,，\n]/)
                .map((s) => s.trim())
                .filter(Boolean),
              readingIds: reading,
              thinkingIds: thinking,
            },
      );
      onDone();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="preference-setup">
      <div className="setup-brand wordmark">
        <Mark />
        <span>Memhot</span>
      </div>
      {!catalog ? (
        <>
          {error ? (
            <>
              <Notice
                error={`偏好选项暂时没能加载：${error}`}
                onRetry={() => setReload((n) => n + 1)}
              />
              <button className="text-button" onClick={onDone}>
                稍后设置，先打开应用
              </button>
            </>
          ) : (
            <Loading />
          )}
        </>
      ) : (
        <>
          <div
            className="setup-progress"
            aria-label={`第 ${step + 1} 步，共 2 步`}
          >
            <span className="done" />
            <span className={step === 1 ? "done" : ""} />
          </div>
          <h1>{step === 0 ? "你关注哪些领域？" : "你喜欢怎样阅读？"}</h1>
          <p className="setup-description">
            {step === 0
              ? "选几个感兴趣的方向，之后随时可以调整。"
              : "选适合你的方式，让材料更合你的习惯。"}
          </p>
          {step === 0 ? (
            <>
              <ChoiceGroup
                label="初始关注领域"
                choices={catalog.interests}
                value={interests}
                onValueChange={setInterests}
                disabled={busy}
              />
              <label className="setup-custom">
                还有其他兴趣？
                <input
                  value={custom}
                  disabled={busy}
                  maxLength={500}
                  onChange={(e) => setCustom(e.target.value)}
                  placeholder="自己填几个，用逗号隔开"
                />
              </label>
            </>
          ) : (
            <>
              <h2>阅读方式</h2>
              <ChoiceGroup
                label="初始阅读方式"
                choices={catalog.reading}
                value={reading}
                onValueChange={setReading}
                disabled={busy}
              />
              <h2>希望 AI 帮你做什么？</h2>
              <ChoiceGroup
                label="初始思考偏好"
                choices={catalog.thinking}
                value={thinking}
                onValueChange={setThinking}
                disabled={busy}
              />
              <p className="field-hint">
                这些选择会保存在「个人偏好」，以后也会结合你的反馈更新。
              </p>
            </>
          )}
          {error && <Notice error={error} />}
          <div className="setup-actions">
            <button
              className="text-button"
              disabled={busy}
              onClick={() => (step === 0 ? void save(true) : setStep(0))}
            >
              {step === 0 ? "先跳过" : "上一步"}
            </button>
            <button
              className="primary"
              disabled={busy}
              onClick={() => (step === 0 ? setStep(1) : void save())}
            >
              {busy ? "保存中…" : step === 0 ? "下一步" : "开始使用"}
              <Icon name="arrow" size={17} />
            </button>
          </div>
        </>
      )}
    </main>
  );
}
