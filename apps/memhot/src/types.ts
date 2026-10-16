export interface ModelConfig {
  protocol: string;
  baseUrl: string;
  model: string;
  configured?: boolean;
  apiKey?: string;
}
export interface Profile {
  name: string;
  interests: string[];
  timezone: string;
  digestHour: number;
  researchHour?: number;
  retentionDays: number;
  models?: { text?: ModelConfig; asr?: ModelConfig };
  feeds?: string[];
  search?: { provider: string; configured?: boolean; apiKey?: string };
}
export interface Source {
  id: string;
  title: string;
  kind: string;
  text?: string;
  createdAt: string;
  status: "queued" | "awaiting_audio" | "transcribing" | "ready" | "failed" | "no_speech";
  error?: string;
  analysisStatus?: string;
  analysisMessage?: string;
  analysisCoverage?: {
    includedChars: number;
    totalChars: number;
    complete: boolean;
  };
}
export interface Thought {
  thinkingDate?: string;
  inToday?: boolean;
  id: string;
  category?: string;
  title: string;
  question: string;
  quote: string;
  sourceId?: string;
  assumptions: string[];
  gaps: string[];
  status: "open" | "kept" | "dismissed" | "resolved";
  createdAt: string;
  analysisStatus?: string;
  origin?: string;
  researchEligible?: boolean;
  revision?: number;
  summary?: string;
}
export interface Cognition {
  id: string;
  type: "interest" | "preference" | "goal" | "belief" | "constraint";
  statement: string;
  origin: "manual" | "explicit" | "inferred";
  status: "active" | "pending" | "dismissed";
  confidence?: number;
  evidenceRefs: { sourceId?: string; conversationId?: string; quote: string }[];
  inferenceReason?: string;
  revision: number;
  updatedAt: string;
}
export interface DailySummary {
  id: string;
  localDate: string;
  timezone: string;
  status: string;
  sections: {
    topicId: string;
    title: string;
    summary: string;
    questions: string[];
    sourceIds: string[];
  }[];
  coverage: {
    includedChars?: number;
    totalChars?: number;
    complete: boolean;
    sourceCount?: number;
    pendingCount?: number;
  };
  createdAt: string;
}
export interface Evidence {
  id: string;
  title: string;
  url: string;
  publisher?: string;
  attribution?: { name: string; url: string } | null;
  publishedAt?: string;
  retrievedAt: string;
  excerpt: string;
  stance: "support" | "counter" | "context";
  retrievalKind?: "page" | "metadata_or_abstract" | "search_snippet";
  contentOrigin?: "external_editorial";
  citations?: {
    claim: string;
    quote: string;
    type: "fact" | "inference";
    stance: "support" | "counter" | "context";
  }[];
}
export interface EditorialStory {
  id?: string;
  eventKey?: string;
  title: string;
  summary: string;
  category?: string;
  whyItMatters?: string;
  readingGuide?: string;
  evidenceIds?: string[];
  coverageCount?: number;
  publishedAt?: string;
  tags?: string[];
}
export interface Report {
  id: string;
  contentOrigin?: "external_editorial";
  attribution?: { name: string; url: string };
  title: string;
  kind: "research" | "digest";
  question?: string;
  summary: string;
  body: string;
  thoughtId?: string;
  createdAt: string;
  status: "ready" | "partial" | "incomplete";
  saved?: boolean;
  sources: Evidence[];
  sourceCount?: number;
  publicEditionId?: string;
  cacheStatus?: string;
  editorial?: {
    version?: string | number;
    leadStoryId?: string;
    highlights?: string[];
    sections?: { id: string; title: string; storyIds: string[] }[];
    stories: EditorialStory[];
    flashes?: EditorialStory[];
    overviewStatus?: "ready" | "unavailable";
    metrics?: {
      storyCount: number;
      sourceCount: number;
      publisherCount: number;
      readingMinutes: number;
    };
    coverageNote?: string;
  };
  gaps: string[];
}
export interface Message {
  id: string;
  role: "user" | "assistant";
  text: string;
  createdAt: string;
  sources?: Evidence[];
}
export interface Job {
  id: string;
  kind: string;
  status: "queued" | "running" | "completed" | "failed" | "blocked";
  message?: string;
  createdAt: string;
  retryable?: boolean;
  requiresChargeConfirmation?: boolean;
}
export interface Home {
  profile: Profile;
  sources: Source[];
  thoughts: Thought[];
  reports: Report[];
  jobs: Job[];
  dailySummaries?: DailySummary[];
  conversations?: {
    id: string;
    reportId?: string;
    thoughtId?: string;
    updatedAt?: string;
  }[];
}
export interface Session {
  baseUrl: string;
  token: string;
  userId?: string;
}
export interface CaptureStatus {
  supported: boolean;
  state: "idle" | "starting" | "recording" | "paused" | "error";
  elapsedMs?: number;
  queuedCount: number;
  uploadedCount: number;
  error?: string;
  notificationsAllowed?: boolean;
  notificationGuide?: string;
  asrMode?: "cloud" | "local";
  noSpeechCount?: number;
  blockedCount?: number;
  retryAfterMs?: number;
  localModel?: {
    id: string;
    version: string;
    state: "missing" | "downloading" | "ready" | "error";
    downloadedBytes: number;
    totalBytes: number;
    downloadBytes: number;
    installedBytes?: number;
    error?: string;
    licenseUrl: string;
  };
}
export const defaultProfile: Profile = {
  name: "",
  interests: [],
  timezone: "Asia/Shanghai",
  digestHour: 8,
  retentionDays: 30,
};
export const emptyHome: Home = {
  profile: defaultProfile,
  sources: [],
  thoughts: [],
  reports: [],
  jobs: [],
};
