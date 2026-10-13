# 日报时效修补与部署排空

2026-10-12。本轮只修改服务器和维护工具，Android客户端保持冻结。2026-10-13已随服务器0.3.3部署。

## 已确认的问题

- 公共版面的键只有 UTC 日期。缓存虽然只有6小时 TTL，但同日重新生成仍读首份不可变输入，并复用已完成的付费输出，所以“更新日报”和当晚任务可能继续使用上午材料。
- 公共日报父任务扇出后，子任务创建时间晚于整批研究任务。同用户只允许一个任务运行，日报子任务因此排在历史研究后。15:04:59 UTC 只读线上聚合为：研究 running 1、queued 55；日报 queued 3；公共模型调用 pending 0、completed 12；收据 pending 0、completed 77；额度 reserved 0。没有读取用户文字、录音或日志内容。
- 显式更新直接加入排期中的任务，但没有移除明早的 availableAt；运行中的任务还持有旧 payload 快照。旧版首次交付的 createdAt 又会使用当前时间，阅读归档可能把旧内容显示为新建。
- 初次检索的日期筛选不能约束编辑 Agent 后来读到的网页；老文或日期未知材料也能独立进入最终新闻。

## 最终行为

公共版面使用 UTC 00/06/12/18 点开始的固定6小时窗口。同窗口的不可变输入、模型阶段键和付费幂等保护继续生效；新窗口可重新检索，平台每日公共调用总预算不变。旧版不改键，已保存引用仍可读取。

新窗口没有新增 canonical 原文 URL 时，不重新调用付费编辑，沿用上一版真实 ID 和生成时间。完整检索无新增标为 unchanged，并明确说明没有新原文；来源存在缺口则标为 stale，并说明覆盖不完整，不把抓取失败宣称为完整无新增。候选 URL 元数据随版面保留，未入选候选不会在24小时原文快照清理后重新变成“新材料”；原始网页文字仍按原24小时规则清理。

排期日报和显式更新日报在同用户的研究积压前领取。跨用户轮转、同用户单任务以及过期运行任务的恢复顺序不变。手动更新加入现有 queued/running 任务，立即展示的请求写入持久 payload；扇出和最终事务提交读取当前标记，不重复启动付费任务，也不再等待明早的展示时间。

终稿每条故事和快讯必须至少实际引用一份近7天内发表的原文，沿用最多6小时未来时钟容差。未知日期、过期或远未来来源不能独立充当新闻；旧资料仍可和近期原文一起作为背景。故事日期按解析后的真实时间比较，来源原始日期仍保留。版面 coverageNote 显示截至时间、原文发表区间、近24小时数量、1–7天背景数量及未核实日期数量。

目前只把新的 canonical URL 视为新增。稳定 URL 上的发布修订、正文更新以及同一事件的不同 URL，后续需要明确的事件/修订标识；本轮不会用导航、抓取时间等动态内容自动制造付费新版本。无相关材料或日期未知时不保证成刊，也不以旧材料填充今天的新进展。

## 部署排空工具

`ops/memhot/maintenance-queue.py` 必须由服务器 root 调用。它只推迟 queued 任务的 next_at，不取消 running，不改变任务 attempt、付费状态或用户记录。原 next_at 和运行中任务后来扇出的子任务/重试都由同一 SQLite 事务和持久触发器保护。恢复时仅改仍处于 queued 且 next_at 仍匹配该 hold 的任务；后来的更长延迟保留。

排空最多3600秒，默认900秒。即使操作者未恢复，原服务器也会在 deadline 后重新领取任务；不能据此在 deadline 后继续认定处于排空状态。SQLite 维护表可保留为空供复用。私有 receipt 包含恢复标识及初始任务原时间，必须保留；后续更新采用临时文件、fsync、原子替换和目录 fsync，提交后更新失败仍能用 prepared receipt 恢复。release 拒绝软链接、非普通文件、错误属主和非私有权限。

操作顺序：

1. 在 Nginx 临时阻止新的 `/api/` 请求并 reload。已经建立的请求继续完成；仅 hold 后台队列不能阻止新的 ASR、讨论等直接请求。
2. 设置足够的 systemd 停止等待时间，再创建私有阶段目录及全新 receipt 路径。不要覆盖已存在 hold 的 receipt。
3. 执行 hold，再检查 status。必须同时满足 holdActive=true、quiescent=true；该状态检查计入 running jobs、reserved usage、pending receipts、pending public calls 和 pending discussions，不输出用户 ID、文本或凭据。
4. 停止旧服务、切换候选、启动服务。新的 fanout/retry 在 hold 有效期内仍会被推迟。
5. release 原 receipt，恢复临时 Nginx 门禁，再核验服务。失败回滚也应先保留 receipt，按同样规则恢复，不能重放结果不明的付费请求。

以下是操作模板，未经父任务实施不代表线上已修改：

```sh
install -d -m 0700 /opt/memhot/maintenance
python3 maintenance-queue.py hold --database /var/lib/memhot/memhot.sqlite --receipt /opt/memhot/maintenance/queue-drain-UNIQUE.json --ttl 1800
python3 maintenance-queue.py status --database /var/lib/memhot/memhot.sqlite
python3 maintenance-queue.py release --database /var/lib/memhot/memhot.sqlite --receipt /opt/memhot/maintenance/queue-drain-UNIQUE.json
```

## 验证

行为回归覆盖6小时/跨日边界、同窗口复用、55项研究积压、用户轮转与过期任务恢复、queued/running 手动加入、扇出竞态、旧版真实日期、候选 URL 元数据跨原文保留期、动态工具来源的终稿日期门槛。维护工具8项 SQLite 测试覆盖新 fanout、失败重试、新延迟保留、私有 receipt、提交后 receipt 更新失败恢复及有界暂停；所有测试只用合成数据，不调用生产模型。

### 原文日期补验

真实模型生成的报告发现 arXiv RSS 收录日期可晚于首次提交。现优先识别原文 `citation_date`，覆盖后到的 RSS 日期；`citation_online_date` 和修改时间不会将旧论文变成新稿。回归以 10 月 8 日首发、10 月 12 日入流为例，按背景材料而非近24小时新内容统计。

维护status将超过1小时的discussion pending仅计入historicalUnknownDiscussions。它们属于历史未知结果的幂等保护，不删除、不重新调用；时间缺失或无法解析仍阻塞排空。此判断必须与API门禁、旧Nginx worker完全退出和其他活跃计数为零一起使用。handler本身受720秒外层时限约束。
