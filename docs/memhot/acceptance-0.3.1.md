# Memhot 0.3.1 安装体验交付

日期：2026-10-12。客户端版本 0.3.1 / versionCode 1004，继续使用已部署的 0.3.0 Pi 研究服务。

## 本次范围

包含用户最新编辑的 Account.tsx 与 RecordingPanel.tsx 文案。录音提示中未完成的条件表达式补上 `: null`，保留用户移除的提示为空；未改写用户措辞。原 0.3.0 下载产物保留，新包使用独立版本地址。

## 验证

- TypeScript 检查通过，前端 6 项测试通过，Vite 和 Tauri Android release 构建成功。
- 签名、非调试标记、ARM64 架构以及 ZIP/ELF 16 KiB 对齐检查通过。
- 模拟器覆盖安装成功，显示 0.3.1 / 1004、target API 36，应用进程启动正常；检查的最近 AndroidRuntime 错误日志中没有 Memhot 崩溃。
- 构建前后前端源文件摘要一致：`f9c600bb24df5a107941eff4ea49b3fbac95cabd37567b17d26600f11e0bf75a`。摘要按 src/public 下排序后的路径与字节内容计算。
- Nginx 配置检查及 reload 成功，线上健康检查返回 `ok / 0.3.0`。
- 服务器经公网 HTTPS 地址完整下载返回 200，43,881,058 字节，SHA256 与本地产物一致。这只验证该网络路径；本次没有进行一加 Ace 3 真机长时间录音、耗电或用户手机网络下载验收。
- 已只读验证现有资料认领邀请有效、未使用；注册密码由用户自己设置。邀请码仅存于用户本机私有说明文件，未写入仓库或公共下载目录。
- 本次未改变研究后端，因此未重复执行上一版的全部后端/SDK 测试；其结果见 [0.3.0 验收](acceptance-0.3.0.md)。

## 下载和恢复

下载：https://memhot.angelkang.com/downloads/memhot-0.3.1-arm64.apk

SHA256：`e909e5fc2f42c67cda47c39aaf0e6e1ea962f24a0c1dc6cd20a64307ca38dc11`

发布证据目录：`/opt/memhot/deployments/apk-0.3.1-20261012T0715`。Nginx 旧配置保存在 `old-nginx.conf`；如需撤回新下载路由，恢复到 `/etc/nginx/sites-available/memhot.angelkang.com`，通过 `nginx -t` 后 reload。未更新服务二进制或数据库。客户端不建议卸载降级，以免丢失本地待同步录音；修复应使用相同签名和更高 versionCode。

构建仍有既有的 Android Gradle 旧接口与 Rust 条件导入警告，不影响此次产物生成；升级 Gradle 主版本前需按构建报告复核 Tauri 插件兼容性。
