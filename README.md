# Memhot

从日常对话中留下想法，第二天收到相关的研究材料和兴趣日报。

这是独立的 Memhot 仓库，包含应用、服务端、研究运行时和部署工具。当前应用版本为 **0.3.3**，服务端版本为 **0.3.8**。Android 已有交付；其他平台尚未完成录音能力适配。

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `apps/memhot` | React 阅读界面及 Tauri 应用 |
| `backend/apps/memhot-server` | 账号、转写转发、个人偏好、研究、日报与讨论 |
| `services/memhot-agent` | Pi SDK 会话、检索、网页与论文工具 |
| `plugins/memhot-capture` | Android 持续录音、语音检测、本地转写与同步 |
| `plugins/memhot-updater` | Android 应用内更新 |
| `backend/crates` | 服务端直接或间接依赖的 5 个共享模块 |
| `ops/memhot` | Android 构建准备、服务部署、备份和更新发布 |
| `docs/memhot` | 产品说明、架构决策、接口与验收记录 |
| `artifacts/memhot` | Memhot 的历史验收截图及公开发布校验信息 |

## 本地开发

需要 Node.js **22.19 或更高版本**、npm 和 Rust stable；当前验证工具链为 Rust **1.94.1**。Android 构建另外需要 Java 17、Android SDK 36 和 NDK 29，见[构建说明](ops/memhot/README.md)。

```sh
npm --prefix apps/memhot ci
npm --prefix services/memhot-agent ci --ignore-scripts
cargo build -p memhot-server --locked
```

启动服务端和界面，分别在两个终端执行：

```sh
MEMHOT_DATA_DIR="$PWD/.data" cargo run -p memhot-server --locked
```

```sh
npm --prefix apps/memhot run dev
```

默认界面地址为 `http://localhost:1420`，服务端地址为 `http://127.0.0.1:8790`。界面默认连接托管服务；本地界面开发不会自动创建本地账号。完整的本地服务配置、邀请账号和模型配置见[服务端文档](docs/memhot/server.md)与[模型运行时说明](docs/memhot/agent-sdk.md)。真实研究需要运营方配置模型与检索服务，密钥不随仓库提供。

## 检查

```sh
npm --prefix apps/memhot test
npm --prefix apps/memhot run build
npm --prefix services/memhot-agent test
cargo test -p memhot-server -p model-runtime -p asr-service \
  -p audio-contracts -p agent-contracts -p provider-config --locked
```

默认测试不调用付费模型。带 `#[ignore]` 的真实服务测试需要单独配置，不能把普通测试通过当作真实模型内容质量验收。

## 文档

- [产品与使用说明](docs/memhot/README.md)
- [最新服务端验收与已知限制](docs/memhot/acceptance-0.3.8.md)
- [架构](docs/memhot/architecture.md)
- [部署与回滚](ops/memhot/README.md)
- [独立仓库的提取范围](docs/memhot/standalone-repository.md)
- [第三方组件和授权说明](docs/memhot/THIRD_PARTY.md)

代码源自原 Memocat 工作区内开发的 Memhot。这里只保留 Memhot 及其必要依赖，不包含原项目的界面、CLI、记忆系统或提交历史；来源及提取边界记录在独立仓库说明中。第三方代码保留各自的授权声明，本仓库不因此自动适用某个统一开源许可证。
