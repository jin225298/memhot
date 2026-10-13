# 独立 Memhot 仓库

2026-10-13 从 `KainingLiu/shike` 的 `adf9530d841847079bbc5364ae6ebc51599d14d9` 提取，目标仓库为 `jin225298/memhot` 的 `main`。独立仓库使用全新的根提交，原仓库及其历史保持不变。

## 范围

保留 `apps/memhot`、`backend/apps/memhot-server`、`services/memhot-agent`、两个 Memhot 插件，以及对应的 `docs/memhot`、`ops/memhot`、已跟踪的 `artifacts/memhot`。保留目录层级，以维持原生插件引用、服务端运行时发现和部署脚本的相对路径。

从应用、服务端和插件的 Cargo manifest 递归解析本地路径依赖，最终只保留 9 个 workspace member。其中共享模块为：

| 模块 | 保留原因 |
| --- | --- |
| `model-runtime` | 服务端模型调用和供应商协议处理 |
| `provider-config` | `model-runtime` 的供应商配置契约 |
| `agent-contracts` | `model-runtime` 使用的协议模型 |
| `asr-service` | 云端 ASR 提供商请求与响应处理 |
| `audio-contracts` | `asr-service` 使用的音频格式模型 |

这些模块保留原有名称、代码和归属上下文。它们是 Memhot 当前真实依赖，不依赖原 Memocat 应用继续存在。

不包含旧 `frontend`、Memocat / VoiceAgent CLI、记忆系统、其他后台模块、原项目实验、Comet 配置或旧项目提交历史。不复制密钥、数据库、APK、模型权重、生成的 Android 工程和依赖缓存。

## 构建边界

根 Cargo workspace 及 workspace dependencies 按实际依赖收敛。锁文件从原锁文件裁剪，保留仍被使用的依赖版本。Rust 工具链选用已验证的 stable，不再继承旧工程默认安装 Windows target 的配置；Android target 由 Android 构建环境安装。

保留的业务代码不因提取而改变。历史验收文档中的服务器路径、本机私有证据路径及当时版本均是历史记录，不是当前仓库的新部署结果。真实服务测试不附带私有密钥、录音或生产资料；需要运行时应显式提供自己的输入。

这次仓库提取不部署服务器、不重签 APK，也不改动线上数据库。正常发布仍须遵循现有的验收、备份和回滚流程。

## 独立目录验证

- 前端重新安装锁定依赖后，19 项测试及生产构建通过。
- Pi 运行时重新安装锁定依赖后，66 项测试通过，1 项需要外部回放输入的测试跳过。
- 服务端与 5 个共享模块：363 项测试通过，7 项真实外部服务测试保持默认忽略。
- 原生应用及两个插件在当前 macOS 主机上 `cargo check --offline --locked -p memhot-app` 通过；没有据此声称重新构建或验证 Android APK。
- 运维脚本：26 项测试通过，13 项需要额外环境的检查跳过。
- Cargo 依赖由 700 项收敛至 521 项；保留依赖没有升级版本。
- 458 个保留的业务、测试、发布和归属文件与原提交逐字节一致，修改限于独立仓库配置、锁文件及说明文档。

验证使用本机已有的 Cargo 编译缓存，但源码和工作区解析均来自新的独立目录；前端与 Pi 的依赖安装在各自的新目录中。没有复制旧应用代码来满足编译。
