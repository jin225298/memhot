# AIHOT MCP 接入（0.3.3）

AIHOT 是公开 AI 资讯的发现渠道。运营方已确认获得适用书面授权；在线服务条款与开源项目的 MIT 许可分别适用。运营开关 `MEMHOT_AIHOT_ENABLED=1` 启用，未设置或其他值均关闭。关闭后不向模型提供 AIHOT 工具，也不执行公共日报的 AIHOT 初始发现；官方 RSS 与公开原文检索继续运行。无需用户填写 API key 或凭证表。

官方说明：[MCP](https://aihot.news/agent?tab=mcp)、[OpenAPI](https://aihot.news/openapi-v1.json)。唯一端点是 `https://aihot.news/api/mcp`，匿名只读 Streamable HTTP。使用精确依赖 `@earendil-works/pi-mcp` 1.1.0 的 `McpClient`、`StreamableHttpTransport`，不自建 MCP 协议客户端。

## 工具与作用域

模型只看到 Rust 自定义的 `aihot` 工具，其 action 映射至七项官方工具：

| action | 官方工具 | 用途 |
| --- | --- | --- |
| latest | aihot_get_latest | 24 小时精选发现 |
| search | aihot_search | 7 天公开主题搜索 |
| hot | aihot_get_hot_topics | 热点及可继续读取的事件 ID |
| story | aihot_get_story | 本轮 hot 返回的事件 |
| daily | aihot_get_daily | AIHOT 日报 |
| weekly | aihot_get_weekly | AIHOT 周报 |
| monthly | aihot_get_monthly | AIHOT 月报 |

公共 AI 日报在模型选材前读取 daily，材料不足时补 latest；返回的有界编辑概览也进入初始模型上下文。公共 AI 工具参数均由服务端固定，模型不能传入私人 query 或指定日期。机器人目录仅允许固定 `robotics embodied intelligence` 搜索。其他公共目录不暴露 AIHOT。story 只接受同一轮 hot 已返回的公开 ID，不接受任意 URL。

私人研究、讨论可以自主使用这些只读动作。搜索只允许公开公司、模型、产品或技术主题，过滤联系方式、网址与私人文本格式；工具说明明确禁止发送私人原文。私人请求及 story 不进入跨用户共享缓存。工具参数校验在 Rust 内再次执行，节点进程不能变更本次授权的工具或参数。

## 来源与缓存

原文 URL 产生稳定来源 ID，保留 AIHOT 发现归属链接；阅读原文后沿用同一 ID 与发现归属。来源详情紧凑显示“发现渠道：AIHOT”，原文标题和原文链接仍指向原作者。聚合摘要统一标为搜索发现且 `fulltext=false`，没有经原文核对的发布日期，不能进入事实证据集合。模型必须通过 `read_source` 取得原文后引用。相同 URL 的官方 RSS 日期与允许使用的官方摘要优先于 AIHOT 发现片段，保留 AIHOT 归属。

daily 的 lead、weekly/monthly 的 headline/overview 与 sections 仅作带 AIHOT 归属的编辑背景，保留日期窗口并有长度限制；没有证据 ID，明确提示不能据此生成事实引文。未知来源、错误归属、无效结构不作成功材料。原文抓取失败、发布日期未知或过旧继续按现有新鲜度与事实审核规则排除，不能拿聚合发布日期补成原文日期。

共享缓存只接收服务端固定公共请求，键由固定工具及参数计算，不含用户、对话、私人搜索词。成功缓存 5 分钟，保留至 1 小时用于受限旧缓存回退；租约和 attempt fencing 防止并发写入。缓存 busy、远端工具错误或运行时暂不可用作为普通工具错误反馈，编辑可改用其他公开来源；协议、权限与持久化错误仍严格失败。

## 隔离与边界

Rust 经共享只读 MCP broker 启动独立 `aihot-main.mjs`，清空环境，不传模型密钥、用户数据或仓库内容。Pi MCP 不加载根目录、扩展、资源发现、本机工具、OAuth 或其他服务器。节点 HTTP 完全经 JSONL RPC 回到 Rust，由 Rust 校验固定 HTTPS 地址、POST/DELETE、请求头白名单和确切 JSON-RPC 操作。Rust 使用现有公开地址校验、DNS 固定、禁代理与禁重定向传输。

并发为 1，整次请求含等待限时 30 秒；普通 HTTP 最长 15 秒。最多 7 个节点 HTTP 请求与 9 个主机输出帧，单响应 512 KiB，单节点帧 128 KiB，累计通信 2 MiB，96 MiB Node 堆。关闭会话使用 SDK 一秒取消信号，主机清理 HTTP 限制 900 毫秒；清理超时不会抹去已完成的发现。429/503 保留有界 Retry-After（缺省 60 秒），没有盲重试或无限重连。

## 可重复验证

离线测试无需模型、API key 或网络：

```sh
npm test --prefix services/memhot-agent
RUSTUP_TOOLCHAIN=stable cargo test -p memhot-server --offline --locked
```

夹具包含官方结构的日报来源、日报 lead 及周/月 overview。测试使用真实 Pi MCP SDK，经模拟主机 RPC 完成 initialize/list/call/DELETE，并覆盖一秒清理取消、超限、错误结构、端点/工具/请求头拒绝、固定公共范围、私人缓存隔离、事件 ID 入场、归属、原文证据拒绝、可选工具失败和官方 RSS 元数据合并。

显式实网验收只请求固定公共 AI daily；它创建并删除临时独立数据库，第二次读取必须命中同一缓存键，仅输出工具名、数量和校验布尔值：

```sh
MEMHOT_AIHOT_ENABLED=1 RUSTUP_TOOLCHAIN=stable \
  cargo run -p memhot-server --example aihot_public_qa --offline --locked
```

2026-10-12 已验证真实 Rust → JSONL → Pi MCP → AIHOT 路径：`mcpTool=aihot_get_daily`、`sourceCount=1`、`hasAttributedOverview=true`、`allDiscoveriesNotFulltext=true`、`allHaveAiHotAttribution=true`、`samePublicCacheKeyHit=true`。测试不代表已阅读原文或已生成通过审核的日报。
