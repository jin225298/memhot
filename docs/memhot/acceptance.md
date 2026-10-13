# Memhot 0.1.0 验收记录

验收日期：2026-10-12（Asia/Shanghai）。目标：一加 Ace 3 / Android 16 / ARM64。

## 产品闭环

这次实现围绕“一个尚未想完整的点子”展开：保存原话或带角色的导入对话，结合相邻记录、既有思考与兴趣形成问题，服务器寻找支持、反证和背景材料，次日报告显示出处、未完成项，讨论继续写回同一思考。固定兴趣日报是独立任务。

用户没有提供本轮可用模型密钥。验收使用专用测试账号、合成文字和模拟器输入；没有读取或上传用户的真实私人对话。真实公开网页已抓取，已生成带链接的资料摘录报告。模型未配置时报告保持 `partial`，不会展示虚构的AI结论。

## 可复现验证

| 范围 | 实际验证 | 结果与边界 |
| --- | --- | --- |
| 服务端与抽取后的ASR | `cargo +1.88.0 test -p audio-contracts -p asr-service -p memhot-server --lib --tests --offline` | 72通过、0失败、1真实MiMo测试忽略。覆盖租户隔离、幂等、耐久音频、任务租约、来源删除、保留期、引用核验、付费结果不明及显式重试 |
| 原Memocat调用方 | `cargo +1.88.0 check -p capture-pipeline -p voice-ingest -p memhot-app --offline` | 通过；不是整个原有应用的全量回归 |
| Rust格式 | 对 Memhot、audio-contracts、asr-service、录音插件五个包运行 `cargo fmt --check` | 通过；全仓库检查存在原有无关格式差异，未批量改写 |
| Web前端 | `npm test`、`npm run build` | 6项测试通过，TypeScript与生产构建通过 |
| 页面实际交互 | 412×915与1280×900 | 配对、保存原话、配置兴趣、原话待分析状态和移动导航实测；无模型时未宣称AI分析成功 |
| Android核心 | Kotlin单元测试、队列instrumentation | 6项核心测试、2项设备测试通过；含双队列实例并发恢复/写入，不只测试内存对象 |
| Android后台 | API36 ARM64模拟器，Home、硬件Back、边缘Back、持续300.1秒熄屏、通知停止 | 独立release实测通过：原PID与microphone FGS保持，通知停止后服务消失，UI回到“开始录音”。最终发布散列版本再次通过硬件/边缘Back、15秒熄屏与UI停止复核 |
| 云端部署 | 公网和源站TLS、health、授权/未授权接口、备份恢复检查 | 独立用户、loopback监听、Nginx、内存限额、SQLite备份；不干扰既有业务服务 |

原独立审查的7项问题已修复并复核：ASR截断不能当成功、429/5xx类型、结果不明的付费重试、全部模型上下文来源血缘、繁忙退避、清空账号与音频竞态、安卓跨实例队列锁。独立复核没有遗留P1/P2；实际抓取发现的RSS偏题与聚合平台日期语义另加回归验证。

Android验收曾发现两种不同现象：旧instrumentation夹具关闭Activity时发生native退出异常；正式包根Activity的Back也会结束进程、影响随后锁屏录音。后者通过仓库内MainActivity模板将root Back改为`moveTaskToBack(true)`修复，硬件键和Android16边缘手势都已实测。没有修改Tauri全局退出机制；失败的旧运行保留日志，不计为通过。

最终新增回归覆盖：HN/HNRSS分享日期不能冒充原文发表日期，2000年视频和旧文不能因重新被分享进入新闻日报；有关注领域时过滤无关RSS；自动研究在东京与洛杉矶时区只排前一天的记录，当天00:00、未来或未知日期不自动排，手动研究仍可立即执行。

最终Android布局验收修复了系统手势区遮挡及隐藏文件输入框被通用样式撑宽的问题。记录页四个导航完整位于系统手势区上方，录音条动作没有横向溢出；开始与停止状态均实际截图。阅读原文通过系统浏览器打开，应用阅读状态保留。

最终公网抓取验收（02:20，专用Quality QA账号）：`robotics`兴趣日报得到2份2026-10-08的arXiv研究摘要，日期落在近7天；[Dex-One2Many](https://arxiv.org/abs/2610.12470v1)、[DreamTrue](https://arxiv.org/abs/2610.12468v1)。来源明确为`metadata_or_abstract`，未假装读完论文全文。原页日期不明内容被排除，一个暂不可读取的RSS显示缺口。未配置文字模型，因此`partial`、`claims=[]`；本测试证明公开发现、过滤和诚实降级，不证明模型研究质量。

## 待目标设备与真实服务验证

这些是有明确验收条件的后续事项，不计作已经通过。

| 跟踪项 | 原因与影响 | 关闭条件 |
| --- | --- | --- |
| MEMHOT-001 一加长时录音与耗电 | 模拟器不能代表ColorOS后台策略、麦克风争用或真实电池；能量门限也可能保留背景声 | 一加Ace 3/Android16实测8小时锁屏、通话中断、网络切换，记录电量、片段丢失与队列恢复 |
| MEMHOT-002 Activity退出限制 | 普通Back已修为移至后台；Tauri最后一个Activity销毁仍可能结束进程，划掉最近任务不能保证继续录音；见[上游问题15671](https://github.com/tauri-apps/tauri/issues/15671) | 上游修复或验证过的原生生命周期方案，覆盖划掉任务与再次打开；当前引导要求保留最近任务 |
| MEMHOT-003 模型真实兼容与内容质量 | 协议和故障通过本地HTTP夹具验证，缺少真实密钥，不能据此宣称所有兼容供应商都可用 | 用户配置所选ASR/文字模型后，中文录音→转写→问题→证据→讨论完整走通，核对引文、反证与计费 |
| MEMHOT-004 非Android安装包 | 复用Tauri界面和云端协议，但没有交付Windows/macOS/iOS包，桌面录音明确不支持 | 按实际设备逐平台构建、签名、安装和数据保护验收；iOS另行验证后台音频政策 |
| MEMHOT-005 16KiB真实运行 | ZIP/ELF对齐检查只能证明二进制布局；当前模拟器是4KiB页 | 在16KiB页设备或对应镜像安装启动并验证录音、TLS与原生链接 |
| MEMHOT-006 长时间个人研究质量 | 公开免费入口有覆盖和限流边界，抓到真实页面不代表足以支持商业判断 | 用用户自选10个真实问题盲核来源相关性、证据支持、遗漏问题与次日价值；按结果添加来源连接器 |

网页缓存24小时、失败音频24小时、原始文字保留策略均由服务器定期执行。备份保留最近七次成功副本；主库删除不立即删除历史备份，详见运维文档。

## 发布记录

服务器最终release：`0.1.0-20261012-60d559c4`，运行二进制SHA256：`60d559c4e7328960d8761e2e07319c7a804fcdfb0a8c3f9f7430d541b07c4c07`。热切换前完成备份，公网health=200、未认证home=401、既有QA设备home=200；既有业务服务保持active。

最终APK：[memhot-0.1.0-arm64.apk](https://memhot.angelkang.com/downloads/memhot-0.1.0-arm64.apk)，10,914,847字节，包版本0.1.0 / versionCode 1000，ARM64，minSdk 24 / targetSdk 36，非debug构建。

- APK SHA256：`2ad7f472600494e348f73426aafd45195dd11a890f7045ceb068ee9d621385fb`。
- RSA4096签名证书SHA256：`dff1d69e01b7a39c416b27b9ad968ef01416632c8996c49e4e502a59e67c4356`；apksigner确认v2/v3有效。
- ZIP 16KiB对齐检查通过，原生ELF LOAD对齐已静态检查；真实16KiB运行仍属于MEMHOT-005。
- 主代理独立从公网完整下载，HTTP200、`application/vnd.android.package-archive`、长度与本地SHA256均一致。

正式Kang空间独立于测试账号，初始配置文件位于本机`~/.local/share/memhot/setup.txt`，权限0600；内含15分钟一次性配对码与为同账号重新生成的命令。密钥未预置。

验收结束后已清空两个合成QA空间的兴趣和订阅，保留报告供核验，避免继续触发测试日报采集。最终SQLite在线备份成功，Memhot服务与备份定时器处于active。

实际安装截图位于仓库`artifacts/memhot/`：`release-today.png`、`release-recording.png`、`release-stopped.png`、`release-notification.png`与`release-external-link.png`。本文件不包含配对码、模型密钥或设备token。
