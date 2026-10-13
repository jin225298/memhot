# ADR 0006：Android 应用内更新与发布指针

状态：2026-10-12 实施。首个包含更新入口的客户端版本为 0.3.3 / code 1006；此前客户端仍需手动下载安装这个版本。本文描述设计与验收要求，实际公网版本与 APK 摘要以发布记录为准。

## 用户行为

Android 用户在个人偏好的应用信息区主动检查更新，查看当前版本、可用版本、包大小和更新说明，再决定下载。下载状态与取消属于独立更新流程，不触发录音权限，不写账号与研究数据。点击「立即更新」后，如果仍停留在更新页且应用在前台，下载完成会继续打开系统安装；离开页面则只继续下载，回来后由用户点「继续安装」。录音正在启动或进行中时先征求停止录音的确认，离开页面会取消尚未发出的安装操作。

首次从 Memhot 请求安装其他 APK，Android 8 及以上可能要求在系统设置允许此来源；允许后仍由系统安装界面确认。应用不会申请设备所有者或 root 权限，不承诺静默安装。该系统来源许可见 [PackageManager.canRequestPackageInstalls](https://developer.android.com/reference/android/content/pm/PackageManager#canRequestPackageInstalls())，系统安装交互见 [PackageInstaller](https://developer.android.com/reference/android/content/pm/PackageInstaller)。

## 信任与固定边界

原生插件仅从 `https://memhot.angelkang.com/updates/android-arm64.json` 获取更新 manifest，禁止 feed 重定向；客户端 API 配置不能替换更新域名。schemaVersion 固定为 1，包名为 `com.angelkang.memhot`，ABI 为 `arm64-v8a`，下载 URL 必须精确匹配该域名下的 `/downloads/memhot-<version>-arm64.apk`。版本必须为数字三段，versionCode 必须高于已安装版本；manifest、APK 字节数和耗时都有上限。

SHA-256 检测完整性，不能单独证明发行者身份。生产信任锚是当前已安装 APK 的签名证书集合。下载后并在交给系统安装前，核对实际字节数与 SHA-256、归档包名、versionName、versionCode、minSdk，以及与当前已安装应用完全相同的签名证书集合；任何不一致都停止安装。证书读取参考 [SigningInfo](https://developer.android.com/reference/android/content/pm/SigningInfo)。当前不接受签名轮换；将来若要支持，必须另行审计 Android 的签名历史和轮换证明。

APK 下载前以不跟随重定向的 HTTPS HEAD 核对地址、状态和长度。实际下载使用系统 DownloadManager；其公开 API 无法关闭 APK 下载的重定向跟随，因此不能宣称每一跳都被客户端固定。最终文件仍必须通过摘要、身份和签名校验；系统安装程序保留最终验证与用户确认。这一边界与 feed 的严格禁止重定向分别验收。

## Manifest 与发布

`ops/memhot/android-releases/<version>.json` 保存小型、版本化的更新说明。`sign-apk.py` 完成正式验签、不可调试、ARM64-only、ZIP/ELF 16 KiB 对齐检查后，从最终 APK、Tauri 配置和同版本说明生成 `artifacts/memhot/memhot-<version>-arm64.update.json`。生成器独立比对 APK 的实际包名、版本、最低系统版本和 ABI，避免说明或配置先更新而 APK 仍旧。

公网 feed 包含 `schemaVersion/packageName/version/versionCode/minSdk/abi/url/size/sha256/notes/publishedAt`，Nginx 仅提供新版本 APK、SHA 文件和固定 feed 的精确静态路由。feed 为 `Cache-Control: no-store`；已发布的版本 APK 不可覆盖。

`publish-android.py --stage <root 私有目录>` 只在运维显式运行时发布。先核对活动配置和旧 feed 的摘要，备份二者，再原子写入版本 APK、SHA 文件和候选配置，执行 `nginx -t` 与 reload。公网完整下载 APK 的字节数与摘要、SHA 文件及 API 健康全部通过后，最后原子切换 feed 并核对公网 feed。任一步失败恢复旧 feed 与配置；已写入的不可变 APK 保留，便于审计及同摘要重试。业务进程和数据库不参与此次发布事务。

## 验收边界

工具测试覆盖错误版本、元数据、URL、hash、schema、活动配置冲突、不可变版本，以及 Nginx/公网 APK/feed 校验失败后的恢复顺序。客户端另需覆盖无更新、下载与取消、进程恢复、摘要或签名错误、来源许可、系统安装确认和覆盖升级后数据保留。工具测试不能替代真实正式签名 APK 的升级验收；模拟器证据也不能替代用户真机结果。
