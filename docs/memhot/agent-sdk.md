# Memhot Pi agent runtime

Memhot embeds the official **Pi coding-agent SDK** (`@earendil-works/pi-coding-agent` **1.1.0**) through `createAgentSession`. Interest digests, personal research, report discussion, translation, and review share the same session harness. Rust supplies the workflow instructions, model choice, tool schemas, scope, and budget. Pi owns conversation state, model turns, tool execution, tool error feedback, and continuation. The model can search, read, search again, change its approach, and decide when to write the requested final answer.

The runtime uses Pi's existing OpenAI Chat Completions, Anthropic Messages, and Google Generative AI adapters. It does not implement a model response parser or a fixed research chain. `@earendil-works/pi-ai` is pinned to **1.1.0** for the supported provider adapters and in-memory stores. The lockfile fixes the full dependency graph. Node **22.19.0 or later** is required.

Official references: [SDK and sessions](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md), [custom providers](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/custom-provider.md), and the installed package's exported declarations.

## Running

From the repository root:

```sh
npm ci --ignore-scripts --prefix services/memhot-agent
npm test --prefix services/memhot-agent
node services/memhot-agent/src/main.mjs
```

Rust starts one child process per invocation, clears inherited environment credentials, and keeps stdout/stdin as private JSONL pipes. The executable entry is `src/main.mjs`; `src/runner.mjs` is an imported module. OpenAI-compatible model IDs beginning with `deepseek-` use Pi’s DeepSeek compatibility flags (`max_tokens`, native thinking format, and required assistant `reasoning_content`) because the real endpoint is intentionally hidden from the child. No build step or Pi CLI installation is required. Install scripts are unnecessary for the text-only runtime.

## Isolation

The session uses `SessionManager.inMemory`, `SettingsManager.inMemory`, in-memory credential/model stores, `modelsPath: null`, and a custom empty `ResourceLoader`. It loads no repository instructions, extensions, skills, prompt files, saved global settings, or global credentials. An explicit tool allowlist activates only Rust-supplied custom tools. Built-in coding tools, shell commands, filesystem tools, and automatic MCP/SDK resource discovery are inactive. The separately isolated, opt-in [AIHOT MCP worker](aihot-mcp.md) exposes seven authorized read-only discovery actions through Rust, without enabling arbitrary MCP servers or local tools. Prompt command/template expansion is disabled. The synthetic working directory is `/`; the host repository path does not enter the prompt.

The loader's official `getSystemPrompt()` method supplies the workflow instructions as Pi's custom system preamble. Shared guidance explicitly defers to the workflow output format; evidence limitations belong only where that format permits them. OpenAI-compatible requests use Pi's native `samplingParams` option to send `response_format: {"type":"json_object"}` for digest, research, translation, and review; discussion keeps prose output. Pi's native `getCurrentTools()` resolves transcript tool additions/removals, and tool-free OpenAI requests explicitly set native `toolChoice: "none"`. No provider messages or responses are reconstructed locally. Mocked native requests verify these controls and the complete instructions for tool-free translation/review and restored research final turns across all three providers. Workflow output validation remains necessary because JSON mode does not enforce the workflow's schema or factual grounding.

All HTTP transport is replaced with a closed RPC fetch function. Pi's Google adapter uses the replaced global fetch; the other adapters receive the same function explicitly. Only generated POST requests to a dummy `https://memhot.invalid` origin can reach the model RPC. The child never sends a real network request, endpoint URL, authentication header, or API key to the host. Rust chooses the fixed configured endpoint and injects credentials. Real secrets remain in Rust. The dummy `rpc-only` key exists only to satisfy provider SDK construction.

Provider retries, session retries, background cache warming, automatic compaction, analytics, install telemetry, and images are disabled. Every model request goes through the Rust ledger. There are no hidden summarization or retry requests. The current context has a hard bound; reaching it fails explicitly instead of silently dropping evidence. A future compaction feature must use the same counted transport and preserve a durable Pi session boundary.

## JSONL contract

Each frame is one UTF-8 JSON value terminated with `\n`. Stdout contains only protocol frames. The first host frame is:

```json
{
  "type": "start",
  "runId": "opaque-run-id",
  "mode": "research",
  "model": { "protocol": "openai", "model": "configured-model-id" },
  "prompt": "The new user request",
  "instructions": "Workflow instructions and final output requirements",
  "tools": [
    {
      "name": "search",
      "description": "Search scoped public sources",
      "inputSchema": {
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"],
        "additionalProperties": false
      }
    }
  ],
  "maxSteps": 8,
  "maxOutputTokens": 4096,
  "messagesJson": "[]"
}
```

Modes are `digest`, `research`, `discussion`, `translation`, and `review`. Protocols are `openai`, `anthropic`, and `gemini`. Tools may be empty. `messagesJson` is optional and is an opaque JSON string containing **native Pi messages**, including timestamps and protocol metadata, rather than UI chat messages. The host stores this string without parsing or reserializing it; ordered Pi prompt sections must survive exactly. A nonempty `prompt` adds one user message. A checkpoint resume uses an empty prompt and the saved opaque string unchanged.

Model request:

```json
{"type":"model","id":"rpc-1","body":{"model":"configured-model-id","stream":true,"messages":[]}}
```

`body` is Pi's actual provider payload. Rust preserves it and selects the endpoint from the original run configuration. OpenAI and Anthropic use their streaming API. Gemini uses `streamGenerateContent?alt=sse`; its payload follows the native Google schema and does not need a `stream` field.

Rust reads a bounded, complete raw SSE response and returns:

```json
{"type":"result","id":"rpc-1","value":{"body":"data: ...\n\n","contentType":"text/event-stream"}}
```

Rust verifies stream completion before settling the ledger. The child builds an HTTP `Response` from the buffered SSE and Pi's official adapter parses it. The MIME value is normalized to exactly `text/event-stream`. Raw provider error responses are not successful model results.

Tool request/result:

```json
{"type":"tool","id":"rpc-2","name":"search","args":{"query":"question"}}
{"type":"result","id":"rpc-2","value":{"sources":[]}}
```

Host error:

```json
{"type":"error","id":"rpc-2","code":"SEARCH_FAILED","message":"Host diagnostic"}
```

The child never repeats the host diagnostic, which can contain sensitive provider details. Tool errors become a sanitized Pi tool-result message so the model can revise its approach. Model errors end the invocation. Connection errors, timeouts, and protocol violations also end the invocation. Rust independently enforces scope, quotas, idempotency, and unknown-outcome recovery.

## Durable boundaries and privacy

After an assistant turn and all its tool results are finalized, the child emits:

```json
{
  "type": "checkpoint",
  "step": 2,
  "messagesJson": "[]",
  "usage": { "inputTokens": 100, "outputTokens": 30, "totalTokens": 130 },
  "actions": ["search", "read_source"],
  "finished": false
}
```

`step` is 1-based within the current invocation. `maxSteps` is the remaining allowance for that invocation; it is not reset to the original budget during recovery. Rust tracks total model/tool use across invocations. `usage` describes that assistant turn and includes provider-reported cache input tokens. Rust owns authoritative charging.

`messagesJson` encodes the **entire cumulative native Pi transcript**, including the initial user message and Pi system/tool declarations. Replace the previous snapshot with it. The child parses the exact string and restores it through `SessionManager.appendMessage` before creating the session. Rust must not parse or sort its object keys: section insertion order determines the rendered system prompt and must match any paid pending request. Do not prepend the original prompt again, concatenate successive cumulative snapshots, or replace only `agent.state.messages`.

Opaque checkpoint transcripts and raw provider SSE are **private continuation state**, never frontend responses, public action traces, ordinary logs, or debug screenshots. They may contain protocol-required thinking blocks, encrypted signatures, and opaque metadata. Removing them can break Anthropic/DeepSeek continuation or Gemini signed tool calls. Rust binds this state to the account, run, workflow input, model/protocol, and tool scope, and expires retained continuation state after **24 hours**. Its storage must have the same access protections as the private account database. Only sanitized action names/counts and normal usage summaries may enter the visible trace. The final answer contains text blocks only.

A successful final assistant turn emits `finished: true` plus `text` on its checkpoint **before** the terminal frame. If the child crashes between these frames, Rust can use that durable final text without another model request or charge. An intermediate or truncated answer never sets `finished`. Literal DeepSeek DSML tool-call markup in text is also incomplete, even when the provider reports a normal stop: it cannot set a recoverable final checkpoint or become a successful answer in any mode.

Terminal success/failure:

```json
{"type":"complete","text":"The usable final answer","steps":3}
{"type":"failed","code":"STEP_LIMIT","message":"The agent stopped before producing a final answer."}
```

Tool-only exhaustion, an empty answer, truncation, provider failure, context overflow, and transport failure are not success. JSON drafts travel as final text; the Rust workflow validates their requested schema before publishing or saving them.

## Bounds and verification

The child permits at most 32 assistant turns, 16 tool definitions, and 32 tool executions per invocation. The host may impose smaller limits. The last remaining assistant turn is reserved for synthesis: native Pi tool-loadout APIs remove tools before that request. The native request hook appends a system message declaring that tool work is complete and requiring only the workflow's final output format from already provided or retrieved evidence. If tool-result history exists, that final request uses an evidence-writing projection: Pi's native system-message helper retains all current instructions and ordered sections; original user messages remain verbatim; actual tool results become explicitly untrusted native user evidence blocks retaining tool names, call IDs, error status, and unchanged content. Intermediate assistant plans, thinking, and tool calls are absent from this final request. The private SessionManager transcript and checkpoints retain the complete original history, with no projected evidence messages appended. Normal execution and checkpoint restart produce identical projected final provider requests across all three adapters. Initial tool-free translation/review requests use the original context. The projected context remains bounded at 512 KiB. Tool execution is sequential. Tool names and JSON schemas are validated before model access; external references and schema regex execution are disabled. Raw tool arguments are validated before Pi's argument coercion and must fit 32 KiB. Tool schemas fit 32 KiB and have bounded depth/node count.

Host frames fit 8 MiB; raw model SSE fits 4 MiB (including streaming framing overhead); private message context fits 512 KiB. Each child frame fits 1 MiB. Cumulative stdout fits 16 MiB, with 1 KiB reserved for a sanitized failure; repeated cumulative checkpoints and provider requests intentionally count separately. RPC waits, startup, and blocked stdout writes time out after 120 seconds. The full invocation times out after 15 minutes. These are hard ceilings rather than targets.

Tests run the actual Pi session and its three official provider adapters against deterministic fake provider SSE over real subprocess JSONL. They cover repeated search/read/replanning, model-directed tool error repair, provider failure without hidden retries, native private checkpoint restoration, signed reasoning continuity, final checkpoint recovery fields, tool isolation, argument/call budgets, frame bounds, RPC correlation, timeouts, and sanitized errors. The subprocess tests use a 128 MiB Node heap and include a multi-MiB, 8192-delta provider SSE answer. No live provider credentials or paid calls are needed. An optional `MEMHOT_AGENT_QA_DB` regression reads the private recorded QA checkpoint without printing or saving its contents and verifies exact paid-request replay offline; the old QA fixture conversion is test-only, with no production compatibility shim.

## Pi web package

Search and page extraction use **`pi-web-access` 0.38.0**, a community package by Nico Bailon (`nicopreme`) listed in the [official Pi package catalog](https://pi.dev/packages/pi-web-access). Its [upstream implementation](https://github.com/nicobailon/pi-web-access) remains unmodified. Exact `jiti` **2.7.0** loads the installed `pi-web-access/web-tool-core.ts` and calls its actual `createStandaloneWebToolCore()` API. No source is copied and no extension host or global Pi resource discovery is activated.

Rust starts `src/web-main.mjs` separately for each tool call, with a 128 MiB Node heap and cleared environment. The worker creates a private temporary configuration below `MEMHOT_WEB_WORKDIR`, removes inherited credentials/proxies, and redirects `globalThis.fetch` to Rust JSONL RPC before importing the plugin. Jiti's transform disk cache is disabled; the standalone core keeps result state in memory for this invocation. Normal completion and failure remove the private configuration; Rust's temporary-directory guard also removes it if the process is killed.

The only enabled core operations are `webSearch` and `fetchContent`. Search uses explicit `degoog` or `searxng`, ten results, and no content prefetch or summaries. Rust selects Degoog first and may try its configured loopback SearXNG endpoint after an empty result or outage within the shared deadline. Other plugin providers, including its default Exa route, are inactive. The standalone core has no Pi model access, curator, or auto-summary hook.

Page reads use the plugin's `readable` mode and HTTP-only extraction routing. Git clone, YouTube, local video, PDF, image, browser-cookie, and remote hosted extraction features are unavailable. GitHub repository/page routes are rejected by the worker; Rust may first convert a validated GitHub blob URL into its deterministic `raw.githubusercontent.com` URL. The worker sends only the authorized original HTTP request; Rust validates public addresses, pins DNS, and revalidates redirects. The plugin's own DNS preflight remains an additional check. Non-success status and challenge pages stop the call, without a fallback provider or denial bypass. Request headers are limited to Accept, Content-Type, and a fixed read User-Agent; no authentication or cookies enter this channel.

Worker start frames:

```json
{"type":"start","action":"search","query":"public topic","provider":"degoog"}
{"type":"start","action":"search","query":"public topic","provider":"searxng","endpoint":"http://127.0.0.1:8080"}
{"type":"start","action":"read","url":"https://sqlite.org/serverless.html"}
```

Credential parameters, arbitrary endpoints, and additional input fields are rejected. Each HTTP operation crosses the host boundary:

```json
{"type":"http","id":"rpc-1","url":"https://degoog.org/api/search","method":"POST","headers":{"accept":"application/json","content-type":"application/json"},"body":"{\"query\":\"public topic\",\"type\":\"web\",\"page\":1}"}
{"type":"result","id":"rpc-1","value":{"status":200,"contentType":"application/json","body":"{\"results\":[]}","url":"https://degoog.org/api/search"}}
{"type":"complete","sources":[{"title":"Document title","url":"https://example.org/document","text":"Extracted text","fulltext":true}]}
```

Host errors use the model worker's existing `error` frame shape and are sanitized. Final sources are derived from the plugin's `structuredContent`; model-visible presentation strings are never parsed as a substitute. Search results have `fulltext: false`; successful page extraction has `fulltext: true`, meaning a retrieved page rather than a search snippet. This flag does not guarantee a complete paper. Worker text is capped at 48,000 characters and Rust applies its material storage limit. The effective source URL follows the host-approved final redirect.

The worker runs at most 45 seconds. Host frames fit 4 MiB, HTTP bodies fit 2 MiB, child frames fit 256 KiB, and cumulative child output fits 8 MiB with reserved failure space. Actual plugin subprocess tests exercise Degoog and SearXNG API payloads, an [official SQLite page](https://sqlite.org/serverless.html) fixture, readable HTML extraction, raw GitHub text, final redirect identity, isolated configuration cleanup, denied/challenge responses, bounds, and credential/tool restrictions. Tests replace DNS preflight and supply HTTP fixtures over RPC, with no live model or HTTP calls from Node.
