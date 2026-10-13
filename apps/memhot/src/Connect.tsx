import { useState, type FormEvent } from "react";
import { Api, defaultServer, errorText } from "./api";
import { saveSession } from "./native";
import { Mark, Icon } from "./Icon";
import { Notice } from "./components";
import type { Session } from "./types";
import { useBackAction } from "./useBackAction";

export function Connect({
  onConnect,
}: {
  onConnect: (session: Session) => void;
}) {
  const [mode, setMode] = useState<"login" | "register" | "reset">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  useBackAction(mode !== "login", () => { if (!busy) changeMode("login"); });
  function changeMode(next: typeof mode) {
    setMode(next);
    setError("");
    setNotice("");
    setPassword("");
    setCode("");
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const api = new Api({ baseUrl: defaultServer, token: "" });
      const deviceName = /Android/i.test(navigator.userAgent)
        ? "Android"
        : "Memhot 客户端";
      if (mode === "reset") {
        await api.request("/api/v1/auth/reset", "POST", {
          email: email.trim(),
          password,
          resetCode: code.trim(),
        });
        changeMode("login");
        setNotice("密码已重置，请使用新密码登录。");
        return;
      }
      const result = await api.request<{ token: string; user: { id: string } }>(
        `/api/v1/auth/${mode}`,
        "POST",
        {
          email: email.trim(),
          password,
          deviceName,
          ...(mode === "register"
            ? { inviteCode: code.trim(), name: name.trim() || "我" }
            : {}),
        },
      );
      if (!result.token || !result.user?.id)
        throw new Error("登录响应不完整，请重试");
      const session = {
        baseUrl: defaultServer,
        token: result.token,
        userId: result.user.id,
      };
      await saveSession(session);
      setPassword("");
      onConnect(session);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="connect-page">
      <div className="connect-intro">
        <div className="wordmark">
          <Mark />
          <span>Memhot</span>
        </div>
        <h1>
          想法先记下，
          <br />
          明天接着想。
        </h1>
        <p>
          把日常对话里还没想完的事留下。
          <br />
          明天，带着新的证据和材料继续。
        </p>
        <div className="intro-flow">
          <span>
            <Icon name="mic" /> 留下原话
          </span>
          <i />
          <span>
            <Icon name="book" /> 阅读证据
          </span>
          <i />
          <span>
            <Icon name="chat" /> 接着讨论
          </span>
        </div>
      </div>
      <form className="connect-form" onSubmit={submit}>
        <h2>
          {mode === "login"
            ? "回到你的思考"
            : mode === "register"
              ? "开启你的个人空间"
              : "重置登录密码"}
        </h2>
        <p className="muted">
          {mode === "register"
            ? "邀请测试中。已有记录的邀请会保留原来的空间。"
            : mode === "reset"
              ? "邀请测试期间，请向管理员领取一次性重置码。"
              : "用邮箱登录，继续昨天没想完的事。"}
        </p>
        <label>
          邮箱
          <input
            type="email"
            required
            maxLength={254}
            autoComplete="email"
            autoCapitalize="none"
            autoCorrect="off"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </label>
        {mode === "register" && (
          <label>
            怎么称呼你
            <input
              value={name}
              maxLength={60}
              autoComplete="nickname"
              onChange={(e) => setName(e.target.value)}
              placeholder="你的名字"
            />
          </label>
        )}
        <label>
          {mode === "reset" ? "新密码" : "密码"}
          <input
            type="password"
            required
            minLength={mode === "login" ? undefined : 12}
            maxLength={128}
            autoComplete={
              mode === "login" ? "current-password" : "new-password"
            }
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={mode === "login" ? "输入密码" : "至少 12 位"}
          />
        </label>
        {mode !== "login" && (
          <label>
            {mode === "reset" ? "一次性重置码" : "邀请码"}
            <input
              type="password"
              autoComplete="one-time-code"
              required
              maxLength={256}
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
        )}
        {error && <Notice error={error} />}
        {notice && (
          <p className="success-message" role="status">
            {notice}
          </p>
        )}
        <button className="primary wide" disabled={busy}>
          {busy
            ? "请稍候…"
            : mode === "login"
              ? "登录"
              : mode === "register"
                ? "接受邀请并注册"
                : "设置新密码"}
          <Icon name="arrow" size={18} />
        </button>
        <div className="auth-links">
          <button
            className="text-button"
            type="button"
            disabled={busy}
            onClick={() => changeMode(mode === "login" ? "register" : "login")}
          >
            {mode === "login" ? "有邀请码？创建账号" : "已有账号，去登录"}
          </button>
          {mode === "login" && (
            <button
              className="text-button"
              type="button"
              disabled={busy}
              onClick={() => changeMode("reset")}
            >
              忘记密码
            </button>
          )}
        </div>
        <p className="privacy-copy">
          录音由你主动开启，随时可以停止。默认云端转写，本机模型可自行选装；文字用于话题整理与个性化研究。测试充值不会产生真实扣款。
        </p>
      </form>
    </div>
  );
}
