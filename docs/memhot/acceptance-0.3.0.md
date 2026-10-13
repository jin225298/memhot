# Memhot 0.3.0 验收

日期：2026-10-12。版本以签名 APK、服务二进制和锁定的 Node SDK 一并交付。正式升级状态以文末部署记录为准。

## 本地与真实模型

- Rust 后端：159 项通过、1 项默认忽略的联网检查另行通过。
- Pi SDK / 网页插件：43 项通过，1 项可选旧私有夹具跳过；三个供应商协议使用真实 SDK 适配器的离线回放检查。
- 前端：类型检查、构建与 6 项边界测试通过。
- 真实 DeepSeek：正常任务队列生成个人研究（9 条逐字引文支撑的结论、3 份官方资料、有证据缺口时保留 partial），围绕报告继续检索讨论、正文中文翻译，以及带总览/重点/栏目/来源的机器人公共日报。详见 [真实模型记录](reading-live-qa.md)。
- UI：桌面与 412×915 手机宽度检查真实日报，无横向溢出。研究引用编号映射另有先失败后通过的回归测试。
- APK：ARM64、版本 0.3.0/code 1003、target API 36、签名/非调试/16 KiB 对齐检查通过；在模拟器安装、启动，无 AndroidRuntime 崩溃日志。尚未完成一加 Ace 3 真机的持续录音耗电验收。
- 变更文件无已配置供应商密钥；代码文件均不超过 500 行；diff 空白检查通过。

## 依赖和边界

使用官方 Pi coding-agent / pi-ai 1.1.0、官方目录收录的社区插件 pi-web-access 0.38.0、Jiti 2.7.0。插件作者是 Nico Bailon，并非 Pi 团队。依赖只从官方 npm registry 安装，禁止安装脚本。

免费 Degoog 搜索配合托管 SearXNG 回退；原文提取使用插件实际 standalone core。明确拒绝服务或验证页不绕过。Rust 持有凭证和所有网络权限，插件没有模型凭证、Shell 或任意文件读取工具。公共目录材料按版本共享；私人研究与讨论按账号隔离。搜索摘要不冒充已读取正文。

静态编译缓存只包含固定插件源代码，不含查询、网页、会话或配置。发布时生成只读模板，每次工具调用在独立临时目录验证并复制，结束后清理；缺失或失配则使用冷启动。详见 [缓存设计](pi-web-transform-cache.md)。

## 产物

- APK SHA256：`aee9e78b301e4105c1afeaa441855aa918f372d9632b4dbc8f1ae9d07ffb67c6`
- Linux 服务 SHA256：`ff6a558bcf5238b48f69d56219be161003c1ea48f8c0609ec4fcc258957d2b77`
- 发布清单 SHA256：`bd2747a5982077ab78bb9ad09ea0e0af262a926d43088b5968977ab5789a58bb`
- SDK 锁文件 SHA256：`f1a0ad1d76daaa59ac520062aaf60fdb38005699a5ca1ee325ed1354b3bde1a3`

## 部署记录

已切换到 `/opt/memhot/releases/0.3.0-pi-final-20261012`。Linux SDK 43 项通过、1 项可选跳过；发布完整性工具 11 项通过。数据库副本完整性为 ok，账号、资料及对应内容摘要一致。切换前没有排队或运行中的正式任务；最终备份在服务停止后取得。

公网 HTTPS 健康检查返回 `0.3.0 / ok`。服务器通过公网域名完整下载 43,881,058 字节 APK，SHA256 与本地产物一致。此次本地 Python HTTP 客户端收到 403，未绕过该拒绝；公网完整下载验证由已经正常返回 200 的服务器侧客户端完成，不代表所有网络/客户端均可访问。

下载地址：https://memhot.angelkang.com/downloads/memhot-0.3.0-arm64.apk

部署证据和停止边界备份位于 `/opt/memhot/deployments/release-0.3.0-pi-20261012`。必要时运行该目录的 `release-server.py rollback --stage /opt/memhot/deployments/release-0.3.0-pi-20261012`，它会先保存升级后的数据库，再恢复停止边界快照和旧服务。Nginx 单域名配置备份为同目录 `old-nginx-before-030.conf`，如需完整回滚需恢复该文件到 `/etc/nginx/sites-available/memhot.angelkang.com`，通过 `nginx -t` 后再 reload。任何回滚应先检查升级后新增的资料和计费状态，避免丢弃已完成的新操作。
