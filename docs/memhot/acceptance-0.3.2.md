# Memhot 0.3.2 验收记录

日期：2026-10-12。Android versionCode：1005。服务端仍为 0.3.0 Pi，本次无数据库迁移。

## 用户反馈与改动

- 开始录音只提示“操作未完成，请重试”：前端现在规范化 Tauri 的 string rejection；Rust 保留原生受控错误。录音权限回调等待 Activity 恢复；离开应用、停止、换账号或等待超时会取消该次启动。
- Tauri 2.11.5 生成了生命周期 observer，却未注册。MainActivity 直接转发 Activity resume/pause/stop 到 PluginManager，使前台判断与权限等待真正生效。
- 系统返回键先关闭菜单、确认区、编辑页或材料详情，再返回所属页面。只有四个主页面的根层级将任务退到后台，不销毁 Activity 或录音服务。讨论草稿继续保留于 App 内存。
- 日报与深度研究全部采用同样的圆角卡片、底色、边框与间距，取消第一项的特殊样式。
- 讨论输入框使用单层圆角容器，去除 textarea 内层焦点框及手动拉伸角。高度从 44px 自动增长到 168px，之后内部滚动；发送按钮保持 44px。
- Android 容器处理系统栏、刘海与键盘占用区域，并将已处理 inset 清零传给 WebView，避免双重留白。网页端仍用自身 safe-area。

## 证据与范围

- `npm --prefix apps/memhot run typecheck` 通过；Vitest 11/11 通过，覆盖原生错误显示、返回优先级与清理、API 原有契约。
- Android 插件 JVM 测试 9/9；完整 instrumentation 10 通过、1 跳过（可选离线模型下载）。随后更新后的 5 项权限测试全部通过，包括两种授权回调先于 resume、离开应用取消、等待超时、后台继续录音和停止。
- 发布 APK 签名、非 debuggable、ARM64-only、ZIP 与 ELF 16KiB 对齐验证通过。
- 同一个已签名 APK 在 Android 16 静音模拟器运行 SDK-only `NavigationSmokeTest`：真实 JS → Rust → Kotlin 的错误拒绝、开始、状态、停止全部通过；两次返回分别退出一层，根层返回后 Activity 保留、录音继续；系统栏 padding 与 IME 显示/隐藏后的 padding 验证通过。
- UI 使用真实 Discussion/ReadingArchive 组件与明确的本地测试数据，检查 412px 手机和 1280px 桌面宽度。输入空态、长草稿滚动、发送后收缩、菜单优先关闭和返回层级均通过，无横向溢出。临时预览入口已删除，未进入安装包。
- 样式扫描无问题；本次改动文件均小于 500 行。
- 未接触 OnePlus Ace 3 真机。旧版错误丢失了诊断原因，不能从通用提示认定某一种真机根因；本版已修复发现的错误传播与生命周期缺陷，仍需用户确认真机录音、锁屏及权限行为。

测试日志位于本机 `~/Library/Caches/memhot-tools/`：`recording-032-tests.log`、`permission-start-final.log`、`navigation-032-runtime.log`、`apk-0.3.2-*.log`。截图位于 `artifacts/memhot/qa/reading-cards-0.3.2-mobile.png` 和 `composer-0.3.2-{mobile,desktop}.png`。

## 发布

- APK：`https://memhot.angelkang.com/downloads/memhot-0.3.2-arm64.apk`
- 大小：43,864,674 bytes（43.9 MB / 41.8 MiB），离线转写大模型不在包内。
- SHA-256：`7c14fc047550c78a5d8229ee9c2635ed665d29df0bf16480f93133dd9a148248`
- 使用同一持久发布签名，支持从 0.3.1 覆盖安装，保留账号与本地资料。
- Nginx 仅添加新版本 APK 与校验文件两个精确路由，旧版本保持可下载。发布脚本在修改前核验活动配置和 APK 摘要、备份配置，检查失败则恢复；没有重启业务服务。
- 公网完整下载后，字节数与 SHA-256 均和本地签名包一致；服务健康检查正常。
- 服务端暂存与配置回滚目录：`/opt/memhot/deployments/apk-0.3.2-20261012T120733Z`。

## 后续跟踪

- **ANDROID-032-01：Tauri 生命周期升级检查。** 升级 Tauri 时检查生成 Activity 是否自行转发插件生命周期；若上游恢复转发，删除 MainActivity 的三个显式 override，保留权限等待与真机回归，防止双重通知。当前依据是 2.11.5 生成源码及 R8 未使用 observer 证据。
- **ANDROID-032-02：一加首次授权与锁屏验收。** 请在覆盖安装后确认首次/已有授权启动、锁屏录音和从通知停止。若仍失败，具体中文错误能区分权限、登录、麦克风启动和设备桥接；不要再次吞掉错误或直接放宽后台限制。
- **ANDROID-032-03：启动耗时。** 当前 3 秒启动界限覆盖 VAD 初始化及 AudioRecord。在获得真机耗时证据前不盲目扩大超时；若确认冷启动误判，再调整启动状态及有界等待，并增加冷启动回归。

SDK-only 发布测试采用 Java，避免测试框架或 Kotlin 扩展调用已被发布 R8 删除的共享符号。测试自身不需要为产品保留调试类。运行时使用假配置，先在 WebView 内存保存原配置，finally 停止并清理测试账号队列、恢复配置；只在静音模拟器执行。

Inset 处理参考：[Android 官方 WebView inset 文档](https://developer.android.com/develop/ui/views/layout/webapps/understand-window-insets)。
