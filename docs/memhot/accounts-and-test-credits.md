# 邀请测试账号与测试额度

邮箱仅作为登录标识，当前没有邮件验证码、自动找回邮件或真实支付。注册需要管理员通过既有可信渠道发放的邮箱绑定邀请。UI 与账单响应始终标记 `mode: test`；测试点不对应人民币或模型 token 价格。

## 管理员操作

管理员命令必须在正式数据目录、以服务用户执行。命令输出包含一次性能力码，不写入普通日志、仓库或公开页面。没有 HTTP 管理员注册/充值任意用户接口。

```sh
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server invite USER_EMAIL
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server invite-credits USER_EMAIL 1000
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server invite OWNER_EMAIL --user EXISTING_USER_ID
sudo -u memhot env MEMHOT_DATA_DIR=/var/lib/memhot /opt/memhot/current/memhot-server adminreset USER_EMAIL
```

邀请默认有效7天，可用 `--days N` 指定1至30天。重新邀请相同邮箱或同一待认领用户会撤销先前未使用邀请。重置码有效15分钟，生成新码后旧码失效；交付前由管理员确认账号主人。命令明确说明没有发送邮件。

`invite-credits EMAIL TOTAL_CREDITS` 将该新用户邀请的初始试用额度设为100至5000点，默认仍为100点，不改变邀请码、期限或其他用户。未注册时持久保存到邀请，注册事务一次性发放并记入账单；已注册且仍处于有效邀请试用时只补足与已发放试用额度的差额，保留已消费、预占、充值点和到期时间。相同值重复执行不重复赠送，已发额度不能下调；旧空间认领邀请不支持额外试用赠送。重新创建邀请会恢复默认100点，需要重新设置。此命令仅在服务器运行，没有客户端任意赠点接口。0.3.11新增邀请字段，旧版本会忽略自定义额度；回滚需核对尚未领取的邀请，不能声称旧binary能继续兑现自定义额度。

原私有空间必须用第二条命令指定精确 `users.id`。先备份并核实该ID的记录、任务与设备数量，再发行认领邀请。注册消费此邀请时仅新增凭据、绑定邮箱，不创建同名空间，不替换原 profile、记录、任务或个人模型设置；事务内撤销旧配对码与设备，发放新登录会话。不要按姓名自动匹配、合并账号。既有同名账号不能代替精确ID。

迁移前使用 SQLite 在线备份并校验可恢复。新增表保留旧 `users.id`，账号数据清空不删账单或赠点记录。旧版本不理解新会话约束，回滚前停止服务并按对应schema恢复已验证备份，不能让旧二进制直接服务新账号库。

## 托管模型配置

服务启动时可读取 `MEMHOT_PROVIDER_FILE` 指向的独立JSON文件。必须为普通文件、权限0600、大小不超过64KiB；以运行服务的用户可读。不要将真实密钥写进仓库、APK、用户profile或命令参数。

```json
{
  "text": {"protocol":"openai","baseUrl":"https://PROVIDER_HOST/v1","model":"MODEL_ID","apiKey":"SECRET"},
  "asr": {"protocol":"openai","baseUrl":"https://PROVIDER_HOST/v1","model":"ASR_MODEL_ID","apiKey":"SECRET"}
}
```

单个kind存在时完整使用此托管配置，用户不能用profile中的endpoint替换托管endpoint而携带平台key。该kind不存在才回退同用户自行配置的模型。所有请求继续验证HTTPS、公网目标和响应上限，API只暴露是否托管/已配置，不返回托管配置或key。服务只在启动时读取；修改后需通过正常发布流程重启。

## API契约

请求及响应字段使用camelCase，未知认证字段拒绝。密码12至128字符，邮箱规范化为ASCII小写；不删除邮箱中的点或 `+` 后缀。

| 端点 | 请求 | 响应 |
| --- | --- | --- |
| `POST /api/v1/auth/register` | `email,password,inviteCode,name,deviceName` | `token,expiresAt,user` |
| `POST /api/v1/auth/login` | `email,password,deviceName` | 同注册 |
| `POST /api/v1/auth/reset` | `email,password,resetCode` | 204，撤销全部旧会话 |
| `GET /api/v1/account` | Bearer | user、subscription、quota、plans、topupPackages、transactions、rules |
| `POST /api/v1/billing/topups` | `packageId`、`Idempotency-Key` | transaction、account |
| `POST /api/v1/billing/plan` | `planId`、`Idempotency-Key` | transaction、account |

登录会话30天有效，每账号最多12个。会话只保存token哈希；密码使用Argon2id PHC串。哈希在独立blocking任务运行，同时最多2次，不占SQLite锁。不存在账号的登录也执行密码哈希验证。登录/注册/重置同一标识15分钟最多10次，同进程同数据目录全局120次；生产代理仍应对认证路由配置连接/IP速率限制。应用不信任任意客户端的转发IP头。

## 测试点规则

- 初次创建或迁移的用户只赠一次100点、7天试用。多次打开数据库、清空内容、重新邀请不能重新领取。
- 探索测试套餐300点/30天，深入测试套餐1000点/30天。活动周期升级仅补差额，不重新发整包；活动周期不降级；到期可激活新周期。套餐赠点到期失效。
- 测试充值包为100或500点，无付款页面、支付SDK或真实货币。每天最多5笔，充值后余额（含预占）不得超过5000点。相同幂等键返回原订单，不重复发点；同键不同包返回409。
- 文本模型每次实际调用1点。云ASR每30秒1点，不足30秒按30秒计；静音本地过滤不外发。一次研究可能包含计划、综合、复核等多次调用。
- 外发前在同一SQLite事务中扣可用余额并增加预占，优先用套餐赠点。余额不足返回 `quota_exhausted`，不发起API调用。
- 成功、已调用但截断/无效响应结算预占；明确HTTP拒绝返还。network/timeout等结果不明保留预占并阻止自动重试。用户明确允许重复调用后用新请求ID；旧预占不因这一操作伪装为已返还。
- 返还进入原来使用的额度桶；已经到期的旧套餐赠点不复活，也不进入新周期。幂等结算不重复扣除。
- 账单只保存最小交易元数据，与可删除的记录、任务、receipt正文分开。删除内容、receipt保留期清理和服务重启不取消结果未知的预占，不恢复已消费额度。

`expiresAt`使用Unix秒。`quota`包括available、included、topup、reserved、used、totalGranted；transaction始终包含mode=test。消费流水分预占和结算，结算行的credits=0表示此前已经从可用额度预占，并不代表免费调用。

## 公共日报的平台预算

固定公共领域的共享编辑使用独立 `public_model_calls/public_model_attempts`，不归属某个用户，也不扣首个订阅者的测试点。仅允许托管文本模型和公共来源材料；不得将私人录音、记忆、问题或个人提示传给 `models::public_call`。用户自己的讨论、研究仍走用户额度账本。

`MEMHOT_PUBLIC_DAILY_CALL_LIMIT`默认为20，每UTC自然日最多20次外发尝试；0关闭新调用，上限200。计数在外发前原子持久化，失败也保留计数，进程退出不退回预算。同edition+phase和相同输入复用成功缓存；同ID不同输入返回冲突。pending、网络结果未知、5xx、截断不自动再调用；明确鉴权拒绝、普通4xx或429同阶段最多尝试两次。业务JSON解析失败仍算调用，缓存不会为隐藏失败而自动清掉。外层模型请求限时90秒，公共编辑同时最多一个，和用户模型请求共享总并发上限。

不同用户同时订阅只引用同一公开edition，不将任意用户设置、密钥或隐私上下文用于共享生成。公共模型流水不记录prompt，只存请求摘要、状态、调用次数和公开响应。

公共日报同步使用 `billing::ensure_subscription` 检查套餐/邀请试用有效期，与点数余额无关；有效套餐余额为0仍能同步。到期返回 `subscription_required`，充值余额不能代替有效订阅。此检查不扣费，私人模型调用仍逐次预占。

自定义兴趣日报复用 `editorial::compose_private` 的两轮筛选、写作和事实审核，但四阶段全部使用当前用户任务的模型账本；不会写入公共调用表或共享缓存。短概览须通过独立支持性审核，且提及的故事全部保留；否则由通过故事的标题生成确定性概览。

## 验证

`cargo test -p memhot-server --test accounts_billing`覆盖旧tenant认领、邀请消费、防跨账号、密码重置撤销、并发不透支、结算幂等、订单幂等、未知结果跨重启保持、退款桶和套餐升级差额。所有测试只使用合成数据，不触发真实模型或付款。

0.3.11 的 `invitation_credits` 4项回归覆盖邀请额度跨重启保存、原码领取与防重复、其他账号默认额度不变、已注册账号保留消费/预占/充值、过期/旧空间邀请拒绝及旧schema迁移；与7项账号账单回归共同通过。2026-10-13发布到 `/opt/memhot/releases/0.3.11-20261013T151519Z`，binary SHA256为`0d0d0d767e85326c3123f06db8c9c19dbceb8afe51e3be5c0ecdabf2cba4cede`。发布阶段在私有数据库副本验证既有邀请码、钱包和账单未变，复核未修改的Pi源码、依赖树和运行时摘要后复用0.3.10测试证据。回滚备份及额度核验记录保存在同名`/opt/memhot/deployments/`阶段；Android安装包仍为0.3.5。邀请码、邮箱与配置密钥不写入仓库。
