import { Today } from "./Today";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Api, ApiError, errorText } from "./api";
import {
  captureStatus,
  getLaunchTarget,
  loadSession,
  startCapture,
  stopCapture,
} from "./native";
import { Icon, Mark } from "./Icon";
import { AppFrame, type Tab } from "./AppFrame";
import { Connect } from "./Connect";
import { Loading, Notice } from "./components";
import { Thoughts } from "./Thoughts";
import { Records } from "./Records";
import {
  PersonalPreferences,
  type PreferenceSection,
} from "./PersonalPreferences";
import { PreferenceSetup } from "./PreferenceSetup";
import { useBackAction } from "./useBackAction";
const Reader = lazy(() =>
  import("./Reader").then((module) => ({ default: module.Reader })),
);
const Discussion = lazy(() =>
  import("./Discussion").then((module) => ({ default: module.Discussion })),
);
import {
  emptyHome,
  type CaptureStatus,
  type Home,
  type Report,
  type Session,
  type Thought,
} from "./types";
export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [booting, setBooting] = useState(true);
  const [tab, setTab] = useState<Tab>("records");
  const [preferenceSection, setPreferenceSection] =
    useState<PreferenceSection>("overview");
  const [setupDone, setSetupDone] = useState(false);
  const [sourceId, setSourceId] = useState<string | null>(null);
  const [readingCategory, setReadingCategory] = useState<
    "research" | "digest" | null
  >(null);
  const [discussionDrafts, setDiscussionDrafts] = useState<
    Record<string, string>
  >({});
  const [recordsEpoch, setRecordsEpoch] = useState(0);
  const [thoughtId, setThoughtId] = useState<string | null>(null);
  const [home, setHome] = useState<Home>(emptyHome);
  const [homeReady, setHomeReady] = useState(false);
  const [homeVersion, setHomeVersion] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [flash, setFlash] = useState("");
  const [busy, setBusy] = useState(false);
  const [reportId, setReportId] = useState<string | null>(null);
  const [discussion, setDiscussion] = useState<{
    report?: Report;
    thought?: Thought;
  } | null>(null);
  const [capture, setCapture] = useState<CaptureStatus>({
    supported: false,
    state: "idle",
    queuedCount: 0,
    uploadedCount: 0,
  });
  const api = useMemo(() => (session ? new Api(session) : null), [session]);
  const refreshVersion = useRef(0);
  const researchRequests = useRef(new Map<string, string>());
  useBackAction(Boolean(session && setupDone && (
    discussion || reportId || (tab === "thoughts" && thoughtId) ||
    (tab === "records" && sourceId) ||
    (tab === "preferences" && preferenceSection !== "overview")
  )), () => {
    if (discussion) setDiscussion(null);
    else if (reportId) setReportId(null);
    else if (tab === "thoughts") setThoughtId(null);
    else if (tab === "records") setSourceId(null);
    else setPreferenceSection(preferenceSection === "cognition-list" ? "cognition" : "overview");
  });
  useEffect(() => {
    setSetupDone(false);
    setHomeReady(false);
    setPreferenceSection("overview");
    setSourceId(null);
    setReadingCategory(null);
    setDiscussionDrafts({});
    setReportId(null);
    setDiscussion(null);
    setThoughtId(null);
  }, [session?.token]);
  useEffect(() => {
    let active = true;
    loadSession()
      .then((s) => {
        if (active) setSession(s);
      })
      .catch((e) => {
        if (active) setError(errorText(e));
      })
      .finally(() => {
        if (active) setBooting(false);
      });
    void getLaunchTarget()
      .then((target) => {
        if (active && target.tab === "today") setTab("today");
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);
  const refresh = useCallback(
    async (signal?: AbortSignal, quiet = false) => {
      if (!api) return;
      const version = ++refreshVersion.current;
      if (!quiet) setLoading(true);
      try {
        const data = await api.home(signal);
        if (!signal?.aborted && version === refreshVersion.current) {
          setHome(data);
          setHomeReady(true);
          setHomeVersion((n) => n + 1);
          setError("");
        }
      } catch (e) {
        if (!signal?.aborted && version === refreshVersion.current) {
          setError(errorText(e));
          if (e instanceof ApiError && e.status === 401) {
            await stopCapture().catch(() => undefined);
            setSession(null);
            setHome(emptyHome);
          }
        }
      } finally {
        if (!signal?.aborted && version === refreshVersion.current)
          setLoading(false);
      }
    },
    [api],
  );
  useEffect(() => {
    const controller = new AbortController();
    void refresh(controller.signal);
    return () => controller.abort();
  }, [refresh]);
  useEffect(() => {
    const controller = new AbortController();
    const update = () => {
      if (document.visibilityState === "visible") {
        void getLaunchTarget()
          .then((target) => {
            if (target.tab === "today") {
              setTab("today");
              setReportId(null);
              setDiscussion(null);
            }
          })
          .catch(() => undefined);
        void captureStatus()
          .then(setCapture)
          .catch((e) =>
            setCapture((c) => ({ ...c, state: "error", error: errorText(e) })),
          );
        if (api) void refresh(controller.signal, true);
      }
    };
    void captureStatus()
      .then(setCapture)
      .catch(() => undefined);
    const timer = setInterval(update, 15000);
    document.addEventListener("visibilitychange", update);
    return () => {
      controller.abort();
      clearInterval(timer);
      document.removeEventListener("visibilitychange", update);
    };
  }, [api, refresh]);
  useEffect(() => {
    if (!flash) return;
    const timer = setTimeout(() => setFlash(""), 5000);
    return () => clearTimeout(timer);
  }, [flash]);
  async function record() {
    setBusy(true);
    setError("");
    try {
      if (capture.state === "recording") await stopCapture();
      else await startCapture();
      setCapture(await captureStatus());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function research(thoughtId?: string, kind?: "research" | "digest") {
    if (!api) return;
    setBusy(true);
    try {
      const key = `${thoughtId || "all"}:${kind || "research"}`;
      if (!researchRequests.current.has(key))
        researchRequests.current.set(key, crypto.randomUUID());
      await api.request(
        "/api/v1/research",
        "POST",
        { thoughtId, kind },
        undefined,
        researchRequests.current.get(key),
      );
      researchRequests.current.delete(key);
      setFlash("已加入更新队列，完成后会出现在「阅读」");
      await refresh(undefined, true);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  function navigate(next: Tab) {
    if (next === "preferences") setPreferenceSection("overview");
    setTab(next);
    setThoughtId(null);
    setReportId(null);
    setDiscussion(null);
    setError("");
    window.scrollTo({ top: 0 });
  }
  function openReport(id: string) {
    setReportId(id);
    setDiscussion(null);
    window.scrollTo({ top: 0 });
  }
  function selectThought(id: string | null) {
    setThoughtId(id);
    window.scrollTo({ top: 0 });
  }
  function openDiscussion(context: { report?: Report; thought?: Thought }) {
    setDiscussion(context);
    window.scrollTo({ top: 0 });
  }
  if (booting)
    return (
      <div className="boot">
        <Mark />
        <Loading />
      </div>
    );
  if (!session || !api)
    return (
      <>
        <Connect
          onConnect={(connected) => {
            setTab("records");
            setSession(connected);
          }}
        />
        {error && (
          <div className="toast" role="alert">
            {error}
          </div>
        )}
      </>
    );
  if (!setupDone)
    return (
      <PreferenceSetup
        api={api}
        onDone={() => {
          setHomeReady(false);
          setSetupDone(true);
          void refresh(undefined, true);
        }}
      />
    );
  return (
    <AppFrame
      tab={tab}
      name={home.profile.name}
      loading={loading}
      capture={capture}
      focused={Boolean(reportId || discussion)}
      onNavigate={navigate}
      onCapture={() => {
        navigate("records");
        setSourceId(null);
      }}
      onRefresh={() => void refresh()}
    >
      {error && <Notice error={error} onRetry={() => void refresh()} />}
      {discussion ? (
        <Suspense fallback={<Loading />}>
          <Discussion
            key={discussion.report?.id || discussion.thought?.id}
            api={api}
            context={discussion}
            text={
              discussionDrafts[
                discussion.report?.id || discussion.thought?.id || ""
              ] || ""
            }
            onText={(text) =>
              setDiscussionDrafts((drafts) => ({
                ...drafts,
                [discussion.report?.id || discussion.thought?.id || ""]: text,
              }))
            }
            previousConversationId={
              home.conversations?.find((c) =>
                discussion.report
                  ? c.reportId === discussion.report.id
                  : c.thoughtId === discussion.thought?.id,
              )?.id
            }
            onBack={() => setDiscussion(null)}
            onChanged={() => void refresh(undefined, true)}
          />
        </Suspense>
      ) : reportId ? (
        <Suspense fallback={<Loading />}>
          <Reader
            key={reportId}
            api={api}
            reportId={reportId}
            onReport={openReport}
            onBack={() => setReportId(null)}
            backLabel={tab === "thoughts" ? "返回思考" : "返回阅读"}
            onDiscuss={(report, question) => {
              if (question)
                setDiscussionDrafts((drafts) => ({
                  ...drafts,
                  [report.id]: question,
                }));
              openDiscussion({ report });
            }}
          />
        </Suspense>
      ) : (
        <>
          {tab === "today" && (
            <Today
              api={api}
              onChanged={() => void refresh(undefined, true)}
              home={home}
              onPreferences={() => {
                navigate("preferences");
                setPreferenceSection("interests");
              }}
              category={readingCategory}
              onCategory={setReadingCategory}
              loading={loading}
              busy={busy}
              navigate={navigate}
              onReport={openReport}
              onResearch={(kind) => research(undefined, kind)}
            />
          )}
          {tab === "thoughts" && (
            <Thoughts
              api={api}
              thoughts={home.thoughts}
              reports={home.reports}
              dailySummaries={home.dailySummaries || []}
              timezone={home.profile.timezone}
              selectedId={thoughtId}
              onSelect={selectThought}
              onReport={openReport}
              onChanged={() => void refresh(undefined, true)}
              onResearch={research}
              onDiscuss={(thought) => openDiscussion({ thought })}
              onAdd={() => navigate("records")}
              onSource={(id) => {
                navigate("records");
                setSourceId(id);
              }}
              busy={busy}
            />
          )}
        </>
      )}
      {/* Keep input drafts in memory across navigation; logout unmounts this frame. */}
      <div hidden={Boolean(discussion || reportId) || tab !== "records"}>
        <Records
          key={recordsEpoch}
          selectedId={sourceId}
          onSelect={setSourceId}
          api={api}
          sources={home.sources}
          active={tab === "records" && !discussion && !reportId}
          onThoughts={() => navigate("thoughts")}
          capture={capture}
          recordBusy={busy}
          onRecord={record}
          onChanged={() => void refresh(undefined, true)}
        />
      </div>
      {tab === "preferences" &&
        !discussion &&
        !reportId &&
        (!homeReady ? (
          <Loading />
        ) : (
          <PersonalPreferences
            api={api}
            profile={home.profile}
            refreshKey={homeVersion}
            section={preferenceSection}
            onSection={setPreferenceSection}
            onSource={(id) => {
              navigate("records");
              setSourceId(id);
            }}
            onSaved={() => void refresh(undefined, true)}
            onCleared={() => {
              setRecordsEpoch((n) => n + 1);
              setSourceId(null);
              setDiscussionDrafts({});
            }}
            onDisconnect={() => {
              setSession(null);
              setHome(emptyHome);
            }}
          />
        ))}
      {flash && (
        <div className="toast" role="status">
          <Icon name="check" size={18} />
          {flash}
        </div>
      )}
    </AppFrame>
  );
}
