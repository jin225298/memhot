import { useEffect, useRef, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Loading, Notice } from "./components";
import { Icon } from "./Icon";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

interface AccountData {
  user: { id: string; name: string; email?: string; emailVerified: boolean };
  mode: string;
  subscription: { planId: string; label: string; expiresAt: string | null };
  quota: {
    available: number;
    reserved: number;
    used: number;
    totalGranted: number;
    unit: string;
  };
  plans: { id: string; name: string; credits: number; durationDays: number }[];
  topupPackages: { id: string; name: string; credits: number }[];
  transactions: {
    id: string;
    kind: string;
    label: string;
    credits: number;
    createdAt: string;
    mode: string;
  }[];
}
export function Account({ api }: { api: Api }) {
  const [account, setAccount] = useState<AccountData | null>(null);
  const [loading, setLoading] = useState(true);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [order, setOrder] = useState<{
    kind: "plan" | "topups";
    id: string;
    name: string;
    credits: number;
  } | null>(null);
  useBackAction(!!order, () => { if (!busy) setOrder(null); }, backLayer.editor);
  const operation = useRef<{ signature: string; key: string } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    void api
      .request<AccountData>(
        "/api/v1/account",
        "GET",
        undefined,
        controller.signal,
      )
      .then((data) => {
        if (!controller.signal.aborted) {
          setAccount(data);
          setError("");
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [api, reload]);
  async function placeOrder() {
    if (!order) return;
    setBusy(true);
    setError("");
    const signature = `${order.kind}:${order.id}`;
    if (operation.current?.signature !== signature)
      operation.current = { signature, key: crypto.randomUUID() };
    try {
      const result = await api.request<{ account: AccountData }>(
        `/api/v1/billing/${order.kind}`,
        "POST",
        order.kind === "plan" ? { planId: order.id } : { packageId: order.id },
        undefined,
        operation.current.key,
      );
      setAccount(result.account);
      setOrder(null);
      operation.current = null;
      setNotice("额度ok");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-section account-page">
      {error && (
        <Notice error={error} onRetry={() => setReload((n) => n + 1)} />
      )}
      {loading && <Loading />}
      {account && (
        <>
          <div className="section-heading">
            <h2>{account.user.email || "已有个人空间"}</h2>
            <span className="quiet-badge">邀请测试</span>
          </div>
          {!account.user.email && (
            <p className="editorial-note">
              原有资料已保留。请使用管理员为这个空间生成的认领邀请码，在退出后注册邮箱；普通新邀请会创建另一个空间。
            </p>
          )}
          <div className="subscription-line">
            <div>
              <h3>{account.subscription.label}</h3>
              <p className="field-hint">
                {account.subscription.expiresAt
                  ? `有效至 ${shortDate(account.subscription.expiresAt)}`
                  : "可选择测试套餐，体验完整研究流程。"}
              </p>
            </div>
          </div>
          <dl className="quota-table">
            <div>
              <dt>可用额度</dt>
              <dd>
                {account.quota.available.toLocaleString()} <small>测试点</small>
              </dd>
            </div>
            <div>
              <dt>处理中预占</dt>
              <dd>
                {account.quota.reserved.toLocaleString()} <small>点</small>
              </dd>
            </div>
            <div>
              <dt>已使用</dt>
              <dd>
                {account.quota.used.toLocaleString()} <small>点</small>
              </dd>
            </div>
          </dl>
          <p className="field-hint">
            云端转写每 30 秒使用 1 点，不足 30 秒按 1
            点计。本地转写不消耗转写额度，文字整理仍使用额度。
          </p>
          <h2 className="account-section-title">选择测试套餐</h2>
          <div className="plan-list">
            {account.plans.map((plan) => (
              <div className="plan-row" key={plan.id}>
                <div>
                  <h3>{plan.name}</h3>
                  <p className="field-hint">
                    {plan.credits} 点 · {plan.durationDays} 天
                  </p>
                </div>
                <button
                  className="secondary"
                  disabled={
                    busy ||
                    !!order ||
                    plan.id === "trial" ||
                    plan.id === account.subscription.planId
                  }
                  onClick={() => {
                    setOrder({ ...plan, kind: "plan" });
                    setNotice("");
                  }}
                >
                  {plan.id === account.subscription.planId
                    ? "当前套餐"
                    : plan.id === "trial"
                      ? "仅限首次领取"
                      : "选择套餐"}
                </button>
              </div>
            ))}
          </div>
          <h2 className="account-section-title">补充测试额度</h2>充值
          <p className="field-hint">
          </p>
          <div className="inline-actions">
            {account.topupPackages.map((pack) => (
              <button
                className="secondary"
                key={pack.id}
                disabled={busy || !!order}
                onClick={() => {
                  setOrder({ ...pack, kind: "topups" });
                  setNotice("");
                }}
              >
                {pack.name} · {pack.credits} 点
              </button>
            ))}
          </div>
          {order && (
            <div
              className="order-confirm"
              role="region"
              aria-label="确认测试订单"
            >
              <h3>{order.name}</h3>
              <p>本次增加 {order.credits} 测试点。真实应付金额：0 元。</p>
              <div className="inline-actions">
                <button
                  className="primary"
                  disabled={busy}
                  onClick={() => void placeOrder()}
                >
                  {busy ? "提交中…" : "确认测试订单"}
                </button>
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() => setOrder(null)}
                >
                  取消
                </button>
              </div>
            </div>
          )}
          {notice && (
            <p role="status" className="success-message">
              <Icon name="check" size={16} />
              {notice}
            </p>
          )}
          <details className="billing-history">
            <summary>查看额度记录</summary>
            <ul>
              {account.transactions.map((tx) => (
                <li key={tx.id}>
                  <span>
                    {tx.label || tx.kind}
                    <small>{shortDate(tx.createdAt)}</small>
                  </span>
                  <span>
                    {tx.credits > 0 ? "+" : ""}
                    {tx.credits} 点
                  </span>
                </li>
              ))}
            </ul>
            {!account.transactions.length && (
              <p className="field-hint">还没有额度记录。</p>
            )}
          </details>
        </>
      )}
    </section>
  );
}
