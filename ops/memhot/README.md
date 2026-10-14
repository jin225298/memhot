# Memhot 部署和构建

目标主机 `kangracknerd`：Debian 12 / x86_64，2 vCPU / 3.33 GiB RAM。采用本机构建的独立 Rust binary、SQLite WAL、systemd 和既有 Nginx。服务只监听 `127.0.0.1:8790`；不新增数据库容器，不在服务器编译，不运行本地大模型。

## 资源与现有服务

审计时根盘余 4.2 GiB、journal 占 3.9 GiB；已按授权清理归档 journal 到 472.7 MiB，根盘增至 7.7 GiB。`/etc/systemd/journald.conf.d/80-memhot-budget.conf` 持久设置 `SystemMaxUse=512M`，已重启日志守护进程读取配置。现有业务服务继续运行；没有删除镜像、容器、卷或业务数据。

基础 unit 的保守默认值是256 MiB内存、0.5 CPU、64个任务。0.3.12 托管 drop-in 为聊天、翻译、日报、研究各预留一个执行名额，将内存软/硬上限调整为512/768 MiB、CPU 150%、TasksMax 128；四个真实 Pi 会话加网页 worker 的离线并发证明与边界见 [ADR-0012](../../docs/memhot/ADR-0012-workload-concurrency.md)。部署必须应用 drop-in，不能只更新 binary。备份限制128 MiB / 0.25 CPU。部署后按实际RSS和worker执行情况复核上限，保持磁盘至少2 GiB空闲。Docker标记的reclaimable空间不是可直接删除的业务资产，不运行全局prune。

## 本机构建工具

已安装 Java 17.0.18、Rust stable 1.94.1、Zig 0.16.0、cargo-zigbuild 0.23.4。仓库原有 Rust 1.88.0 pin 保留；已使用独立缓存目录完成 1.88.0 的 Android aarch64 `cargo check --offline --locked`，验证依赖与类型检查可通过。完整 Android APK 构建通过命令级 `RUSTUP_TOOLCHAIN=stable` 使用 1.94.1；没有把类型检查写成 1.88 完整 APK 构建验证。

Android SDK 位于 `/Users/kang/Library/Android/sdk`：

- Command-line Tools 22.0，ARM 下载包 SHA256 已按官网核验。
- API 36 revision 2、Build-Tools 36.0.0、Platform-Tools 37.0.1。
- NDK 29.0.14206865、Emulator 37.2.12、API 36 Google APIs ARM64 image revision 7。
- Rust 1.88.0 / stable 均安装 `aarch64-linux-android` target。
- 独立 AVD `Memhot_API_36_arm64`，HVF 加速可用。普通镜像是 4 KiB 页，不能代替 16 KiB 设备运行测试或一加真机功耗测试。

安装日志：`/Users/kang/Library/Caches/memhot-tools/`。代理仅用于具体命令，不修改用户全局代理或 shell profile。

在仓库根目录构建服务器：

```sh
RUSTUP_TOOLCHAIN=stable cargo zigbuild -p memhot-server --release --target x86_64-unknown-linux-musl
file .target-tauri/x86_64-unknown-linux-musl/release/memhot-server
```

仓库默认 `target-dir=.target-tauri`。本轮为避免与 Android/宿主测试争用锁，命令另设 `CARGO_TARGET_DIR=/Users/kang/Library/Caches/memhot-tools/server-target`；实际 binary 位于该目录的 `x86_64-unknown-linux-musl/release/memhot-server`。

## 服务器文件布局

```text
/opt/memhot/releases/<release>/memhot-server
/opt/memhot/releases/<release>/agent/{src,test,package.json,package-lock.json}
/opt/memhot/releases/<release>/release-manifest.json
/opt/memhot/current -> releases/<release>
/opt/memhot/runtime/node-v22.23.3-linux-x64/
/opt/memhot/backup.py
/var/lib/memhot/memhot.sqlite
/var/lib/memhot/audio/
/var/lib/memhot/providers.json
/opt/memhot/deployments/<private-stage>/
/etc/systemd/system/memhot.service.d/20-invitation-beta.conf
/var/lib/memhot/backups/
/etc/systemd/system/memhot.service
/etc/systemd/system/memhot-backup.service
/etc/systemd/system/memhot-backup.timer
/etc/nginx/sites-available/memhot.angelkang.com
```

服务使用专用 `memhot` 系统用户。私有目录模式 0700，SQLite 和备份文件模式 0600，unit 设置 UMask=0077。0.2.0 的平台模型密钥通过 `MEMHOT_PROVIDER_FILE=/var/lib/memhot/providers.json` 读取，独立文件0600、服务用户可读，不写进仓库、systemd unit 或 APK。同种类模型完整优先采用平台配置，客户端不能替换平台端点。历史用户自带配置仅在该种类没有托管配置时作为兼容回退。

## 发布顺序

1. 本地测试、构建 Linux musl binary，记录 SHA256。上传到新 release 目录，比较远端 SHA256。禁止覆盖仍运行的 binary。
2. 已有数据库时先以 `memhot` 用户运行 `backup.py`；数据库迁移必须先确认备份可恢复。
3. `current` 原子切换到新 release，启动或重启独立 `memhot.service`。
4. 检查 loopback `/healthz`、unit RSS、错误计数与私有接口未授权拒绝。
5. 将本目录 `nginx.conf` 安装到独立 vhost，通过 `nginx -t` 后 reload。验证源站 SNI/证书链和公网 `/healthz`。
6. 运行首份数据库备份，启用 `memhot-backup.timer`。保留前一 release 以便回滚。

TLS 证书由现有 Certbot 账户使用独立 webroot `/var/www/memhot-acme` 签发；证书路径写在 Nginx 配置中，私钥不进入源码。`certbot-deploy-hook.sh` 安装到 `/etc/letsencrypt/renewal-hooks/deploy/20-memhot-nginx-reload.sh`，只在该域名续期时测试并 reload Nginx。

源站证书已签发至 2027-01-09。已通过源站 SNI/证书链验证和公网 `/healthz` 检查；API 响应禁止缓存，下载目录仅公开已验证 APK。准备或维护状态应明确返回 503，公网 200 或 Cloudflare 边缘证书不单独作为源站部署成功的证据。

## 邀请账号、备份与回滚

0.2.0 使用邮箱绑定的单次邀请与密码登录。邮箱当前仅作登录标识，不声称发送验证码或重置邮件；套餐和充值仅使用测试点，不接真实支付。

```sh
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server invite USER_EMAIL
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server invite OWNER_EMAIL --user EXISTING_USER_ID
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server adminreset USER_EMAIL
```

既有空间认领必须带精确旧UID；不能根据姓名新建或合并。管理员输出中的邀请码/重置码仅保存到本机私有0600文件，通过可信渠道交付本人，绝不写进部署日志。用户自行设置密码。原配对仅为0.1.x历史设备兼容保留；一旦认领邮箱账号，旧配对与设备会在同一事务中撤销。

备份通过 SQLite online backup API 读取已提交 WAL 数据，完整性检查后压缩，仅保留最近七次成功备份。备份包含私有数据和模型配置；删除操作的历史副本会保留到这些备份被后续七次成功备份替换。需要立即清除历史副本时，必须同时清理受影响备份。该数量轮替不保证失败或停机期间的七天删除时限。

回滚时停止独立 Memhot 服务，保留当前数据库副本；若 schema 兼容，切换 `current` 到前一 release 后启动。若迁移不兼容，恢复对应 SQLite 备份并移除旧 WAL/SHM，再启动对应 binary。不得把新的数据库交给不兼容的旧 binary。

## 0.2.0 发布准备（历史流程）

`release-server.py` 是显式调用的服务器运维工具，不会自行发布。`prepare` 保存私有在线备份、原release与原配置；`verify` 核对新binary SHA256，在独立备份副本执行 `memhot-server check`，验证迁移后用户/profile/records摘要未变。`check` 不启动worker，也不触发模型请求。`activate` 只在最终测试和发布放行后运行：停止独立Memhot服务、再次备份最终数据库边界、原子切换binary和配置、验证新版本健康。失败时由运维显式运行 `rollback`；回滚先保存升级后的数据库/WAL/SHM，再恢复最终升级前备份及对应旧binary，不让旧binary直接打开新schema。

`hosted-beta.conf` 只含非敏感环境变量：平台配置文件路径、`MEMHOT_SEARCH_URL=http://127.0.0.1:8791`、公共编辑每日20次调用上限。SearXNG独立服务见 [search/README.md](search/README.md)。公共目录日报一次编辑后供多用户引用，不消耗首个订阅者的点数；私人兴趣、讨论、记忆分析走各自用户账本。

服务器准备及邀请码输出保存在本机 `~/.local/share/memhot/deployment/` 的0700目录与0600文件。服务器阶段目录只允许root及服务用户按权限访问，不位于Nginx公开目录。准备期间不切换服务、不公开候选APK。APK发布保留旧版本精确路由，新版本必须通过本地验签、上传hash与公网完整下载hash三重核对。

## 0.3.0 Pi 发布准备

候选目录为 `/opt/memhot/releases/0.3.0-pi-20261012`，私有阶段为 `/opt/memhot/deployments/release-0.3.0-pi-20261012`。准备与校验不停止当前服务、不切换`current`、不将候选APK放入公开目录。旧release、数据库和provider均保留。

官方Node 22.23.3 Linux x64安装在独立`/opt/memhot/runtime/node-v22.23.3-linux-x64`；保留原主机Node20。归档SHA256固定为`df450af89261115ef9f9e3830c3eeb2cc9213b63c720b1af623cb5dcbe2e02de`，校验时同时比较归档内Node binary与安装文件并执行版本检查。`hosted-beta.conf`仅含运行时绝对路径、既有非敏感配置和资源限制，密钥仍在权限受限的provider文件。

上传SDK的`src/`、`test/`、`package.json`及`package-lock.json`，不上传macOS的`node_modules`。`release-manifest.json`为`{"version":"0.3.0","files":{"agent/package.json":"<sha256>","agent/package-lock.json":"<sha256>","agent/src/main.mjs":"<sha256>","...":"..."}}`，必须完整列出全部源码与测试；最终清单再加入`memhot-server`。清单SHA256由本机构建方独立记录，不以服务器自算值替代。源码、依赖和目录必须root所有且服务用户不可写。

`release-server.py`与同目录的`release_bundle.py`一起上传。依次显式运行：

```sh
python3 <stage>/release-server.py prepare --stage <stage>
python3 <stage>/release-server.py sdk-verify --stage <stage> --release <candidate> --manifest-sha256 <SDK-only-manifest-sha256>
# 最终binary到位，更新清单后执行完整校验：
python3 <stage>/release-server.py verify --stage <stage> --release <candidate> --sha256 <binary-sha256> --manifest-sha256 <full-manifest-sha256>
```

SDK校验使用独立Node执行`npm ci --ignore-scripts --include=optional --registry=https://registry.npmjs.org`，只接受官方npm registry及带integrity的锁文件。用户配置与独立空全局配置使用不同路径，避免npm拒绝重复加载同一文件。官方Pi SDK固定1.1.0，社区搜索插件`pi-web-access`固定0.38.0，加载器`jiti`固定2.7.0。随后在清空环境的`memhot`用户下运行全部SDK测试，独立systemd瞬态任务使用256/384 MiB内存、100% CPU、64任务和Node128 MiB堆上限，并禁止真实网络。测试与npm原始输出存入阶段目录0600私有证据文件；校验摘要只输出版本、哈希与资源计数。测试结束仅清理该校验任务，不操作其他服务。

SDK测试回执记录源码摘要、Linux依赖树摘要、Node哈希和测试证据哈希。最终`verify`复用未变化的SDK回执，在私有SQLite副本执行新binary的`check`并确认既有用户/profile/records摘要不变。`activate`会再次核对完整清单、SDK依赖树、运行时与配置哈希，然后保存停服边界备份并原子切换`current`，使binary与SDK入口同步变化。只有最终QA与发布放行后才运行：

```sh
python3 <stage>/release-server.py activate --stage <stage> --version 0.3.0
```

新版本健康失败时显式调用同阶段的`rollback`，恢复对应旧binary、配置及停服边界数据库，并保留升级后数据供恢复。APK另经本地签名验证、私有上传hash及公网完整下载hash核验后才公开；Nginx仅增加0.3的两条精确下载路由，保留所有旧版本路径。

同步讨论实际端点为`POST /api/v1/discuss`；候选Nginx仅对此精确路径设置780秒读取等待，其余API仍为120秒。该配置必须独立通过`nginx -t`，完成服务激活和客户端等待时间匹配后再安装/reload。

`sdk-resource-smoke.mjs <candidate>/agent`用模拟SSE和HTML保持真实Pi会话与真实网页插件同时运行，不调用付费模型。资源验收应在相同384 MiB限额的瞬态任务中执行，记录cgroup的`memory.peak`与`memory.events`；完整SDK测试和该并发冒烟均不能代替真实生产任务峰值监测。

## Android 产物检查

先从仓库根目录运行 `python3 ops/memhot/prepare-android.py`，该脚本会在缺少生成项目时运行 init，再固定 compileSdk/targetSdk 36 和 NDK 29，禁止 Android 自动备份私有配置，复制本地 MainActivity 的根任务返回键处理，以及仅模拟器使用的 instrumentation fixture。随后从 `apps/memhot` 使用 Tauri CLI 构建 ARM64 APK。生成文件保持忽略，本机 Cargo 路径不进入 Git。release 构建必须在前端验收后执行；开发签名与正式签名须区分。

```sh
ANDROID_HOME=/Users/kang/Library/Android/sdk \
NDK_HOME=/Users/kang/Library/Android/sdk/ndk/29.0.14206865 \
RUSTUP_TOOLCHAIN=stable npm exec -- tauri android build --target aarch64 --apk --ci
```

成品使用 SDK 的 `apksigner verify --verbose --print-certs` 和 `zipalign -c -P 16 -v 4` 验证，记录 SHA256。NDK r28+ 默认 16 KiB ELF 对齐，APK 打包需 AGP ≥8.5.1；仍需检查最终产物。模拟器验证安装、启动、权限和录音队列；八小时锁屏、电量和一加后台限制留给目标真机实测。

独立 release 签名存于 `/Users/kang/.local/share/memhot/signing/`（0700）：`memhot-release.p12` 与 `keystore-password` 均为0600，alias `memhot-release`。密码不打印、不提交、不随APK分发；未来原地升级必须保留同一签名。公开下载目录只放已验证APK，私有数据库与签名目录不映射到Web服务器。


## 本轮 Android 验证边界

本地 `MainActivity` 在根任务接收返回时调用 `moveTaskToBack(true)`。Android 16 模拟器已用系统返回键和边缘返回手势验证：Activity 移到后台，录音前台服务继续；再次打开仍正常显示。未修改 Tauri 全局退出行为。Home、返回和锁屏可维持录音；划掉最近任务、强行停止或厂商后台策略可能结束录音，一加真机应保留任务并按应用内说明配置后台权限。

原生队列 6 项 JVM 单元测试和 2 项真实 SQLite instrumentation 测试通过，包含另一实例在写入期间反复 recovery 的并发检查。与最终包原生代码相同的签名候选包经系统返回键、边缘返回、关闭显示连续 300 秒检查，原进程和 microphone 前台服务始终存活；点击通知“停止录音”后服务退出。最终仅修 CSS 的交付包重新完成两种返回、15 秒熄屏和 UI 停止检查。测试使用无宿主音频的模拟器默认输入，没有采集 Mac 现场声音，也不代表真实语音识别或八小时耗电结果。

最终签名使用 `JAVA_HOME=<Java17路径> python3 ops/memhot/sign-apk.py`，脚本同时检查不可调试 release、ARM64-only、APK ZIP 16 KiB 对齐及全部原生 ELF LOAD 对齐，并生成 SHA256 文件。签名升级后还需复核界面安全区、返回和外部来源链接。产物和验收截图放在 `artifacts/memhot/`；详细执行证据在 `/Users/kang/Library/Caches/memhot-tools/`。

## Android 更新 feed 与独立发布工具（0.3.3 起）

先在 `android-releases/<version>.json` 记录同版本的 `schemaVersion: 1`、`version`、`versionCode` 和 1–8 条短更新说明。签名脚本全部检查通过后生成 `artifacts/memhot/memhot-<version>-arm64.update.json`。也可显式运行 `android_release.py --apk <正式签名包> --config <tauri.conf.json> --notes <版本说明.json> --output <update.json>`；独立命令会重新验签，并检查 APK 的实际版本、包名、minSdk24 和 ARM64 库。

服务器阶段必须是 `/opt/memhot/deployments/<唯一阶段名>` 的 root 所有 0700 目录。上传本次签名 APK、同名 `.apk.sha256`、候选 `nginx.conf`，把生成的 update JSON 复制为 `android-arm64.json`；各输入文件须 root 所有且不可被组或其他用户写入。只复制脚本及其相邻依赖 `android_release.py`，不使用缓存中的临时历史发布脚本。

`manifest.json` 的固定结构如下，所有摘要均为完整小写 SHA-256。`oldFeedSha256` 在首次发布、旧 feed 确实不存在时填 `null`，后续填当前文件的实际摘要；活动配置或 feed 改变会阻止发布。准备阶段的 Nginx 基线摘要为 `63435a68a5ab2095c2d3e1efddf8c56cb871ab6f91656ab570a952b26b36bbe2`；执行前仍需读取活动文件确认，不可把历史摘要当成当前状态。

```json
{
  "schemaVersion": 1,
  "version": "0.3.3",
  "versionCode": 1006,
  "apkSha256": "<最终 APK 摘要>",
  "oldNginxSha256": "<当前活动配置摘要>",
  "newNginxSha256": "<阶段 nginx.conf 摘要>",
  "updateSha256": "<阶段 android-arm64.json 摘要>",
  "oldFeedSha256": null
}
```

确认已完成客户端验收后，运维显式执行 `python3 <阶段或受控工具目录>/publish-android.py --stage /opt/memhot/deployments/<唯一阶段名>`。工具持有独立发布锁，保存 `old-nginx.conf` 和存在时的 `old-android-arm64.json`。先原子发布不可变 APK/SHA 文件和配置，通过 `nginx -t`、reload、新静态路由就绪检查、公网完整 APK/校验文件和 `/healthz` 验证后，最后才原子切换 `/opt/memhot/public/updates/android-arm64.json` 并核对公网 feed。HTTP 请求必须直接返回 HTTPS 200，禁止自动跟随重定向。

成功记录 `verified-public-apk.json` 与 `published.json`；失败自动恢复旧 feed 与配置并记录 `rolled-back.json`。若恢复也失败，`rollback-failed.json` 明确记录需运维处理。阶段为一次性使用；失败重试须建新阶段，保留既有证据与备份。版本 APK 不能被不同内容覆盖；已写入的相同摘要 APK 可复用。该工具不重启 Memhot 服务、不迁移数据库。

本地回归：`python3 -m unittest discover -s ops/memhot -p test_android_release.py -v`。客户端信任边界及系统安装许可见 [ADR 0006](../../docs/memhot/ADR-0006-android-updates.md)。


## 0.1.0 历史发布证据（2026-10-12）

- 服务端运行 binary SHA256：`60d559c4e7328960d8761e2e07319c7a804fcdfb0a8c3f9f7430d541b07c4c07`。公网 `/healthz` 200，未认证 `/api/v1/home` 401，QA 认证 home 200。最终检索质量 QA 后 RSS 3888 KiB、cgroup 3760128 bytes、2 tasks；根盘余 7.6 GiB，既有 nginx/docker/memocat-memory 仍 active。
- 公开 APK：`https://memhot.angelkang.com/downloads/memhot-0.1.0-arm64.apk`，10,914,847 bytes，SHA256 `2ad7f472600494e348f73426aafd45195dd11a890f7045ceb068ee9d621385fb`。公网完整下载比较通过。v2/v3 签名有效，证书 SHA256 `dff1d69e01b7a39c416b27b9ad968ef01416632c8996c49e4e502a59e67c4356`。
- 最终 Records 四导航 AX 底部为 2317 physical px，系统导航条从 2337 开始；模拟器 density 420，底部系统 inset 63 physical px = 24 CSS px。隐藏文件控件不再撑宽布局，录音条右边界在屏内。
- 实际“阅读原文”点击以 ACTION_VIEW 打开外部 Chrome 的公开 arXiv URL，私有 WebView 保留在 Memhot。未把第三方页面导航进带原生权限的 WebView。
- 原始证据：`server-quality-acceptance.log`、`server-final-resource.log`、`release-back-screenoff-smoke.log`、`final-release-short-smoke.log`、`release-external-link.log`、`public-apk-verify.log`、`apk-signature-verify.log`、`apk-zipalign-16k.log`、`apk-elf-alignment.log`，均位于上述本机缓存目录。

较早 ActivityScenario instrumentation 的 300 秒/停止断言已完成，但模拟器 EGL/native mutex 在 Activity 自动收尾时异常终止，该次 instrumentation 整体不计通过；其失败日志保留。生命周期验收使用后续真实签名 APK 的返回、熄屏和通知停止证据，未把收尾失败改写为通过。


## 0.1.1 发布验证（2026-10-12）

最终 Android 包为 `memhot-0.1.1-arm64.apk`，versionCode **1001**，10,914,847 bytes，SHA256 `9b19db15337678a3b3d4fde8c57f727a256aa1aba02f60f38cf834009f4cca43`。前端构建包含 `index-wj6rGn6c.js` / `index-C1qLzQ2H.css`。三项主导航为“今天 / 思考 / 记录”，设置入口在右上头像；阅读页面保留完整正文、来源展开和外部原文链接。

同一正式证书 SHA256 `dff1d69e01b7a39c416b27b9ad968ef01416632c8996c49e4e502a59e67c4356` 已验证，v2/v3 签名、不可调试 release、ARM64-only、APK ZIP 16 KiB 和全部原生 ELF LOAD 16 KiB 对齐检查通过。签名脚本从配置读取版本并核对最终 manifest，验证日志按版本保存，不覆盖 0.1.0 的证据。

API36 ARM64 模拟器已实际从 0.1.0/1000 执行 `adb install -r` 升级到 0.1.1/1001，首次安装时间 `2026-10-12 02:05:41` 与原合成 QA 数据保留。最终包硬件返回和边缘返回都让 Activity 移到后台，原 PID 与麦克风前台服务继续；确认屏幕进入 Asleep 后连续计时 **60 秒**，每20秒复核进程、服务和熄屏状态均通过。UI“停止录音”和通知“停止录音”均让 CaptureService 退出。原文页离开后从“录音中”入口返回，立即可见停止按钮。模拟器以 `-no-audio` 启动，未采集宿主现场声音。

实际点击“阅读原文”已打开外部 Chrome 的 ISCA 原文页面。截图检查覆盖今天、记录输入、研究阅读、来源、思考和录音页面；截图只含合成 QA / 公开论文材料，存于 `artifacts/memhot/v0.1.1/android-*.png`。此证据不代表一加真机后台限制、8小时耗电或16 KiB设备运行验证。

最终服务端为 `0.1.1-20261012-baf0bc04`，binary SHA256 `baf0bc0448a4451e8ca4a11d026f3e4971701317432ea4407d8fdc546466114c`。部署前在线备份 SQLite，最新压缩备份已独立解压并通过 `integrity_check=ok`，6张表结构与线上一致；无 schema 变更，旧 release 保留。公网 `/healthz` 返回200和版本0.1.1，未认证 `/api/v1/home` 返回401。发布检查时服务 active、2 tasks、无重启、cgroup约1.2 MB，根盘余7.5 GiB。

本轮原始证据位于 `/Users/kang/Library/Caches/memhot-tools/`：`android-0.1.1-final-build.log`、`apk-0.1.1-{signature-verify,zipalign-16k,elf-alignment,badging}.log`、`android-011-native-smoke.log`、`server-011-final-cross-build.log` 和 `server-011-final-deploy.json`。公开产物元数据在 `artifacts/memhot/v0.1.1/release.json`。


0.1.1 已通过独立精确 Nginx 下载路由公开在 `https://memhot.angelkang.com/downloads/memhot-0.1.1-arm64.apk`；`nginx -t` 和 reload 成功，公网完整下载与本地产物 SHA256 一致，证据为 `public-apk-0.1.1-verify.json`。原0.1.0下载路径和文件SHA保持不变。另用原有纯合成日报完成“兴趣日报→阅读→返回”分类保持检查，没有触发新模型任务。录音验收结束后已停止本轮启动的模拟器，保留AVD、原应用数据和SDK。

## 0.2.0 首次服务器升级证据（2026-10-12）

服务器已部署 `0.2.0-20261012-c3599e04`，静态musl binary为10,822,952 bytes，SHA256 `c3599e042e43a4dc6089217d4017e3a1b69c9b843c7e5b72f42f5dd061fa3c10`。构建使用stable工具链、独立server-target缓存及 `--locked --offline`，包含最终来源删除隐私补丁。

上线前在在线备份副本执行新binary的 `check`，用户/profile/records摘要一致；停服后再次保存最终升级前备份。本机独立恢复验证 `integrity_check=ok`。上线后原空间全部既有记录存在，原raw数据和profile未变化；后台可正常增加新日报记录。托管配置文件0600且归属服务用户，启用loopback搜索与公共编辑每日20次预算。专属认领邀请仅保存在管理员本机私有文件，未替用户认领或设置密码。

公网普通curl验证 `/healthz` 返回200和0.2.0，未认证home/account返回401；默认urllib客户端被既有边缘规则拒绝，不改WAF规避。阶段检查Memhot为active、0次重启、2个任务、cgroup约2.6MiB，既有nginx/docker/memocat-memory保持active。后续真实分析会改变任务数及内存，阶段值不是峰值承诺。

公开服务器元数据在 `artifacts/memhot/v0.2.0/server-release.json`。私有构建、备份、恢复、迁移与激活记录在本机 `~/.local/share/memhot/deployment/release-0.2.0-20261012T021152Z/`；对应服务器私有阶段目录为 `/opt/memhot/deployments/release-0.2.0-20261012T021152Z/`。回滚只在明确运维需要时调用该目录的 `release-server.py rollback --stage <同目录>`，先保存升级后数据库再恢复停服边界备份；不要仅切旧binary直接服务新schema。

首次服务器升级时APK仍待最终UI验收；最终公开包见下文0.2.0 Android发布记录。

## 0.2.0 引文、引导与解析修复发布（2026-10-12）

当前服务器 release 为 `0.2.0-20261012-fcf7efa5`，静态musl binary为10,894,104 bytes，SHA256 `fcf7efa5de6a3b0ef0f1cfaeb8f0a0d30639ad5a89b2e8dfecf0152b383e12e6`。合并后116项服务器测试全部通过，包含首次偏好原子设置、引文可验证修复和重新分析覆盖状态；日期解析保留空格分隔的明确旧发布时间，RSS与media:title仅对完整相同值去重。

上线前确认没有进行中的模型请求或任务，独立备份副本迁移检查通过。激活前最终备份已在本机独立恢复，19张表、`integrity_check=ok`；正式空间9条记录、profile和原认领邀请摘要在切换前后完全相同，账号仍未认领。运行进程实际binary哈希与上传产物一致。公网health为200，未认证account/home/preferences setup均为401。阶段检查0次重启、2个任务、约3.6MiB内存；nginx、docker和memocat-memory保持active。

当前私有发布证据位于 `~/.local/share/memhot/deployment/hotfix-0.2.0-20261012T024631Z/`，服务器同名阶段目录位于 `/opt/memhot/deployments/`。该阶段 `release-server.py rollback --stage <同目录>` 可保存升级后数据并回到上一版0.2.0；首次升级阶段备份仍保留供历史恢复。

生产“人工智能”公共日报验收只使用既有合成QA账号：11条实际候选材料，经两轮独立评分、编辑和审核生成5篇文章，恰好4次平台调用，用户测试点数100保持不变。公共正文保存1份，用户只保存引用；本账号读取200，另一个QA账号读该引用404，直接edition ID读取404。测试兴趣、订阅源已清空，验收后无挂起平台调用或QA任务。本版资料来自NVIDIA、Hugging Face与Google Research主站；部分来源只取得官方订阅摘要，覆盖边界在阅读结果中保留。私有内容与逐步响应仅保存在 `~/.local/share/memhot/qa/public-ai-production-20261012T024117Z/`。

该次后端切换时APK仍等待最终原生验收；随后只发布了下文列明哈希的包，所有较早0.2.0候选均未公开。

## 0.2.0 Android 已验收基线发布（2026-10-12）

已公开 [memhot-0.2.0-arm64.apk](https://memhot.angelkang.com/downloads/memhot-0.2.0-arm64.apk) 与 [SHA256校验文件](https://memhot.angelkang.com/downloads/memhot-0.2.0-arm64.apk.sha256)。包大小43,864,674 bytes，版本0.2.0 / code1002，SHA256 `806455e75762289a112e0212fadde02d1c6981aefa0b5c3ff0153986471f994e`；沿用原正式签名证书。

本包为最终组件UI、四导航、个人偏好内认知管理、账号顶部卡和首次选择引导的已验收基线。签名、ARM64-only、ZIP与ELF的16KiB对齐及原生交互检查证据位于 `artifacts/memhot/v0.2.0-qa/final-components/`。后续报告生成与内容展示改进另行构建发布，不包含在当前下载包中。

发布时先上传至私有staging并核对哈希，再加入APK和SHA文件两条精确Nginx路由；`nginx -t`、reload成功。公网完整下载43,864,674 bytes后SHA256与本地及服务器文件完全相同；校验文件200且内容正确。旧0.1.1下载返回200，源文件SHA256未改变。Memhot应用PID未变，发布过程未重启应用、未修改用户数据或触发模型调用。

公开元数据为 `artifacts/memhot/v0.2.0/release.json` 和 `server-release.json`；私有发布与公网下载证据位于 `~/.local/share/memhot/deployment/apk-0.2.0-20261012T033241Z/`。需撤回此包时，恢复该阶段保存的原Nginx配置并测试/reload即可移除新下载路由，保留旧版路由及私有发布证据；无需回滚应用数据库。
