# 第三方设计与许可

日报编辑流程参考 [AIHOT](https://github.com/KKKKhazix/AIHOT)，参考版本 e36480d548aaa1c8461dc93b7b940559b6dd7099（2026-10-12）。其两轮独立筛选、事件归并、结论优先摘要、每日编排与来源层级启发了 Memhot 的独立实现。MIT 许可全文见 [AIHOT-MIT.txt](licenses/AIHOT-MIT.txt)。这份开源许可不代表 AIHOT 在线服务数据的使用授权。

0.3.3 新增 [AIHOT 官方 MCP](https://aihot.news/agent?tab=mcp) 发现渠道。产品运营方已确认获得适用书面授权；部署仅在 `MEMHOT_AIHOT_ENABLED=1` 时启用。接口归属保留为 AIHOT，原文链接与原文作者归属保留；聚合摘要、日报/周报/月报编辑概览不能冒充已读原文或事实引文。具体作用域、缓存与关闭方式见 [AIHOT MCP 接入](aihot-mcp.md)。原有独立实现及官方 RSS、公开原文检索仍可单独运行。

MCP 客户端直接使用 Pi 官方 `@earendil-works/pi-mcp` `1.1.0`（MIT）。锁文件固定官方 npm 注册表地址与 SHA512 完整性值；运行包保留依赖自身许可证。客户端代码不复制进 Memhot，服务端以独立进程和受控 RPC 传输调用其 API。

离线语音运行时、VAD 与 SenseVoice 模型的独立归属与许可见 [capture THIRD_PARTY](../../plugins/memhot-capture/THIRD_PARTY.md)。公开源网站内容仍属于各自权利人；Memhot保留必要引用、阅读链接和短期处理缓存。

界面组件直接采用并调整 [shadcn/ui](https://github.com/shadcn-ui/ui) 的 Select、Accordion、Dialog、Toggle Group 组合，克隆版本 `02cad61166c4c20edbc8a3f71bd720eed4c1f49b`，来源 `apps/v4/registry/new-york-v4/ui`。组件源码位于 `apps/memhot/src/ui`；将 Tailwind 样式映射到现有 CSS 变量，复用现有 SVG 图标，键盘、焦点、弹层和选择逻辑由官方 Radix primitives 执行。许可全文见 [shadcn-ui-MIT.txt](licenses/shadcn-ui-MIT.txt)。不复制 React Bits Pro 付费源码。

官方 Radix UI primitives（Select、Accordion、Dialog、Toggle Group）为 MIT 许可，见 [Radix-MIT.txt](licenses/Radix-MIT.txt)。shadcn/ui 和 Radix 许可同时打包至客户端 `third-party-notices.txt`。

服务端检索直接安装并执行 [pi-web-access](https://github.com/nicobailon/pi-web-access) `0.38.0`，作者 Nico Bailon，MIT 许可见 [pi-web-access-MIT.txt](licenses/pi-web-access-MIT.txt)。该项目由 [Pi 官方扩展目录](https://pi.dev/packages/pi-web-access) 收录，属于社区扩展，并非 Pi 官方团队编写。Memhot 调用其独立执行核心，复用搜索、网页正文提取及结构化结果；服务器约束网络、用户作用域和保存时间。不启用其编程、浏览器账号、视频或额外模型总结能力。

0.3.7 的 PDF 提取复用现有锁文件中 `pi-web-access` 已安装的 [unpdf](https://github.com/unjs/unpdf) `1.8.1`（MIT），许可全文见 [unpdf-MIT.txt](licenses/unpdf-MIT.txt)。独立 Worker 只提取公开文档的文本层，保留页数和截断信息，不执行 PDF 脚本，不启用 OCR。服务运行包保留依赖及其随附许可。
