# 微信公众号订阅接入调查

调查日期：2026-10-13。代码快照：RSSHub `adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee`；WeRSS `0862aab90d7aea91e776bcdab4b495a727f12051`。范围：只读核查官方文档、维护方文档/代码与 Memhot 现有实现；未安装转换器，未扫码、登录、调用付费 API 或部署。这里给出接入设计，尚未实现公众号接入。

## 结论

可分期支持，但不能承诺“任意公众号、免登录、完整历史、实时更新”同时成立。建议先做**文章链接导入 + 经核验的公开 RSS**；后续试验**用户自己管理的私有连接器**；确有公众号运营者合作时，再做**官方授权接入**。

官方 API 可读有权限的自有号/授权方号，普通读者的微信登录或关注行为不会赋予 Memhot 读取任意公众号后台的权限。文章公开可读，也不意味着登录会话、私有订阅关系或整站全文分发可以共享。Wechat2RSS 私有软件有明确商业用途/内容分发限制，排除作为 Memhot 多用户云端采集后台。

## 官方能力边界：自有号与任意第三方号

本轮通过普通 HTTPS GET 成功读取微信原站当前文档；网页检索工具对这些页面报错，未通过镜像补证。

- [`freepublish/batchget`](https://developers.weixin.qq.com/doc/subscription/api/public/api_freepublish_batchget.html)：已成功发布消息列表，`offset` 分页，`count` 1–20，默认可返回正文。支持第三方平台代商家调用，权限集 ID 为 **7**，使用 `authorizer_access_token`。
- [`freepublish/getarticle`](https://developers.weixin.qq.com/doc/subscription/api/public/api_freepublishgetarticle.html)：按 `article_id` 获取已发布图文，返回正文及图文信息；同样支持权限集 7 的授权方调用。
- 两页当前“适用范围”表均列“公众号：仅认证；服务号：✔”。页面将“仅认证”解释为企业主体已认证账号。具体账号还须在公众平台开发者中心核实实际接口权限；错误 `48001` 明确指向功能未授权。**本轮未找到可核验的微信原站 2025-07 权限回收公告，故不把该历史日期作为结论。**
- [`access_token`](https://developers.weixin.qq.com/doc/subscription/api/base/api_getaccesstoken.html) 来源为账号的 AppID/AppSecret；[`authorizer_access_token`](https://developers.weixin.qq.com/doc/oplatform/openApi/ticket-token/api_getauthorizeraccesstoken) 属于已授权账号，有效期 2 小时，可用该授权方 refresh token 更新。它不是消费者微信账号的关注列表权限。

因此，官方路线适合自己运营或运营者明确授权的号，无法靠“用户扫码登录微信”扩大到所有其他公众号。正式接入还需核验开放平台第三方平台资质、授权入口、撤销事件及实际账号权限；本轮未注册平台或实调接口。发布接口列表也不能直接解释为完整的公众号历史消息列表：群发、后台发布、旧素材以及删除文章的覆盖必须用授权测试号逐一验证。

## 路线比较

| 路线 | 可得到什么 | 登录、历史与正文 | 频率、成本与维护 | Memhot 判断 |
|---|---|---|---|---|
| 用户分享文章链接，或粘贴有权使用的正文 | 指定单篇；适用于任意用户已能阅读的公开文章 | 无需把微信会话交给 Memhot；没有自动发现或历史回补。普通请求若遇验证、删除、付费/权限页，保留链接和失败原因；可让用户手动粘贴正文 | 无转换器软件费用；按需提取，维护成本最低；原文可访问性仍会变化 | **第一期基线**，但要明确只是导入文章，不能显示为已订阅整个公众号 |
| 公众号作者自有 RSS/网站同步源，或获准使用的公开 RSS；RSSHub 的特定公开路线 | 已覆盖的号/合集的新条目 | 作者源通常不需微信会话；第三方源只覆盖其索引。历史上限由源决定；正文可能只有摘要。RSSHub 合集路线只包含合集内文章 | 共用轮询成本低；第三方收录延迟、停服与反爬会影响结果，没有通用 SLA | **第一期自动订阅候选**，逐源探测和标明覆盖，不承诺任意号 |
| 用户自己的 WeRSS 私有实例 → 私有 RSS/API | 会话允许查询到的公众号；列表、分页及可选正文 | 依赖公众号后台扫码 cookie/token，过期需用户重新授权；历史能力依模式/页数/起始时间而定，不能保证完整 | MIT 软件本身无许可费；Python/FastAPI + Playwright + DB，另有服务器、升级和会话维护成本。不能将低频当成免风控保证 | **第二期试验候选**。先接用户实例的只读输出，不把用户微信会话集中到 Memhot 云端 |
| 官方自有号/运营者授权号 API | 授权方可调用范围内的已发布文章、正文 | AppSecret 或第三方平台授权；短 token 可刷新，无需模拟后台登录。只有获授权账号；完整历史/群发覆盖待测试 | 官方额度、账号资质和平台接入成本；通常维护边界最清楚。没有核实出单独的读取接口收费，不等于无账号认证成本 | **合作场景优先**；不解决普通用户订阅任意第三方号 |
| Wechat2RSS 公开服务/付费私有软件 | 维护方收录号；私有软件称支持任意号和全文 | 私有软件用微信读书扫码。当前 QA 明确只抓最新 20 篇、不抓历史、仅收录群发；存量查询不是补抓历史 | 私有软件 15 元/月或 150 元/年，需自行部署；每日检查约 1–2 次、延迟 0–24h，风控后退避可到 6h。协议禁止商业用途、公开服务和二次分发 | **排除云端后台方案**；公开 feed 也不能仅因 URL 公开就推定允许 Memhot 商用分发 |

### 公开 RSS 与 RSSHub 的实际限制

RSSHub 是转换框架，不是微信官方通用订阅 API。当前仓库有以下实际路线：

- [公众号文章合集/话题](https://github.com/DIYgod/RSSHub/blob/adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee/lib/routes/wechat/msgalbum.ts)：无需 cookie 配置，但要求已知 `biz` 与合集 ID，只抓页面中的合集条目；代码标记 `antiCrawler: true`，不代表全号订阅或完整历史。
- [CareerEngine 来源](https://github.com/DIYgod/RSSHub/blob/adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee/lib/routes/wechat/ce.ts)：依赖该第三方已收录公众号；当前实现只取 feed 前 10 项，再读第三方文章页。作者未被收录就无法订阅。
- [搜狗来源](https://github.com/DIYgod/RSSHub/blob/adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee/lib/routes/wechat/sogou.ts) 仍存在，但代码依赖搜索结果与 cookie，不能据此承诺稳定、无授权的通用入口；[新榜路线](https://github.com/DIYgod/RSSHub/blob/adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee/lib/routes/newrank/wechat.ts) 明确要求 `NEWRANK_COOKIE`，不列入第一期无会话来源。

RSSHub 使用 [AGPL-3.0](https://github.com/DIYgod/RSSHub/blob/adb82c30b251d1645884d8b5c1c9b6ea7b6e43ee/LICENSE)。如未来修改并通过网络向用户提供该软件，需评估其源码提供义务；读取一个现成 RSS 不等于把 RSSHub 代码嵌入 Memhot。软件许可证也不授予上游文章的再分发权。本轮未部署 RSSHub，未验证目标公众号的线上路由可用性。

[Wechat2RSS 免费列表](https://wechat2rss.xlab.app/list/) 只公开部分公众号，维护方表示尽力在 24h 内收录。本轮读取公开目录的普通实时请求返回 403，因此不声称已验证 feed 健康或最新文章；未继续尝试绕过访问限制。[QA](https://wechat2rss.xlab.app/deploy/qa)、[价格](https://wechat2rss.xlab.app/deploy/)、[用户协议](https://wechat2rss.xlab.app/deploy/agreement.html) 是上述产品边界依据。其 [API](https://wechat2rss.xlab.app/deploy/api) 有查询存量文章、添加订阅与账号状态接口，但 `k` 查询参数承担鉴权，不应进入客户端、日志或共享 URL。

### WeRSS：锁定仓库与会话边界

本调查的 WeRSS 指 [`letuswerss/werss`](https://github.com/letuswerss/werss)，与 `rachelos/we-mp-rss`、`liangfushou/werss` 及同名在线站点分别看待，不推定维护方、许可与收费相同。

- [README](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/README.md) 定位为个人私有自托管，建议关闭正文、全文 RSS、图片转存和导出；[LICENSE](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/LICENSE) 为 MIT。未发现强制软件购买要求，但软件许可不能替代微信平台或内容方许可。
- [采集代码](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/core/wx/base.py) 从配置读取 cookie/token，调用 `mp.weixin.qq.com` 后台接口，缺 token 时要求扫码登录公众号平台。它与官方 `authorizer_access_token` 是两种权限来源。公开服务部署适配性尚未成立。
- [配置](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/config.example.yaml) 默认为 app 模式、首次/定时最大 1 页，逐稿件间隔 60s；支持 web/api/app，不同模式产生发布链接、临时链接或最新消息。全文 RSS 默认关闭，但当前配置中的 `gather.content` 与补正文开关实际默认 **True**，与 README 保守示例不一致，试验时必须显式关闭/设定。
- 配置里的 `token_expire_minutes=4320` 是 WeRSS 自身登录会话，**不能解释为微信会话固定三天有效**。[cookie 代码](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/driver/cookies.py) 检查 `slave_sid` 过期时间；[token 持久化代码](https://github.com/letuswerss/werss/blob/0862aab90d7aea91e776bcdab4b495a727f12051/driver/token.py) 保存到 `data/wx.lic` 并打印 token。未来隔离运行、日志脱敏与凭据加密需要审查，不能原样嵌入云端。

另外，常见的 [`cooderl/wewe-rss`](https://github.com/cooderl/wewe-rss) 已于 2026-05-11 归档。其 README 宣称历史/全文、微信读书扫码、失效重登，并依赖第三方转发服务；MIT 不消除维护与会话风险，因此不选为长期主方案。

## 与 Memhot 当前代码的接点

当前代码已有 RSS 与来源证据能力，尚无完整的“公众号连接器/订阅状态”模型：

- `profile.rs` 的 `feeds` 是用户配置的 URL 数组，上限 20；`discovery.rs` 每次轮转读取最多 3 个 feed。仅把公众号 RSS URL 放入数组，不能可靠覆盖多个订阅的更新与失败状态。
- `feed.rs` 可解析 RSS/Atom 的标题、正文文本、链接和发布日期，一次最多 20 条，但 `Material.fulltext` 为 false；不能因 RSS 提供长文本就直接宣称全文已核验。
- `sources.rs` 的来源为私人 note/import，并会进入个人想法形成任务。公众号文章应作为**外部参考材料**导入，不应被默认视为用户自己的陈述而提取个人想法。
- `shared_public.rs` 仅接受服务端固定目录来源，并拒绝查询参数/凭据；Wechat 常见 `__biz/mid/idx/sn` URL、私有 feed、带 `k` 的链接不能直接塞入此缓存。此边界应保留，通过独立的规范化文章注册层适配。
- 当前 `freshness.rs::canonical` 只去除片段和少数跟踪参数；微信短链接、长链接与临时访问参数尚不能自动归并。已有 `publishedAt`、`retrievedAt`、`discoveredVia`、正文可用性及缓存状态，可复用于公众号证据。

## 建议的多租户模型与接入边界

以下为设计建议，不是已实现行为：

1. **源与订阅分离。** `source_catalog` 存公众号稳定身份、显示名、公开源种类、覆盖声明及 provider；`source_subscriptions(user_id, source_id)` 存用户订阅、是否进入日报、筛选、加入时间与个人阅读进度。订阅关系和用户分析始终按用户隔离。
2. **连接器单独授权。** `source_connections` 记录 owner、provider、secret_ref、权限范围及健康状态；私有实例只允许拥有者使用。新增订阅前检查连接器所有权，抓取任务按连接器权限执行。公共源与私人实例同号也不能合并授权范围。
3. **只共享独立确认的公开材料。** `public_articles` 按微信文章身份去重，长链接优先使用 `(biz, mid, idx)`，保留 `sn` 与原始 URL 的溯源映射；短链接仅在普通合法重定向/页面元数据足以确认后合并，不按标题猜测。正文 revision/content hash 分开，避免改稿覆盖旧引用。
4. **凭据不能随内容升级为公共。** 经私人会话获取的文章默认 tenant_private；相同 URL 在另一条无登录公开路径独立核验且使用权限合适，才可登记公共版本。不能用用户 A 会话去满足用户 B 的读取、刷新或历史请求。仅跨用户共享合格的文章与公共编辑结果，用户兴趣、订阅、分析、私有查询不进入公共编辑键。
5. **微信会话留在用户实例。** 优先仅在 Memhot 服务端保存连接器的只读 API key/secret_ref，不收微信 Cookie。确有正式需要时，凭据须按租户加密、最小读取权限、可撤销、轮换；不存 `profile.feeds` 明文 token，不进入 prompt、Pi worker通用上下文、导出、日志或公共缓存。连接器请求由独立工具代理，以不含密钥的源 ID 调用。
6. **公网抓取仍走现有网络校验。** 私有实例地址、重定向与 DNS 均需 SSRF 防护；不能为了支持用户自托管放开任意内网地址。确需本地实例时另设计出站推送/安全通道，不让云端通用 fetch 获得内网权限。

## 更新、失败与日报新鲜度

每个源维护 `lastAttemptAt / lastSuccessAt / newestPublishedAt / earliestCoveredAt / nextRetryAt / lastError / coverage`。列表成功但正文失败必须分开记录。UI 展示“最近检查”“最近文章”“仅摘要/正文可用”“历史范围”，不能用抓取时间改写发布日期。

- 普通公开 RSS 初始可建议 6–12h 轮询，采用 ETag/Last-Modified、源级缓存和共享抓取租约；最终频率按来源条款及实测调整。低频不会自动赋予授权或保证不触发限制。
- “主动刷新”先返回已有结果并发起有上限的源级任务；同源请求合并，冷却/退避仍生效。若 RSS 供应方只每日更新，Memhot 刷新只检查供应方，不显示为微信全站刷新。
- 状态区分 `ready / pending / stale / auth_required / rate_limited / access_restricted / no_new_articles / partial / removed`，保留旧内容日期并显示明确缺口；遇验证码、登录或访问限制就停止依赖抓取并提示用户处理，不尝试绕过。
- 首次订阅默认纳入未来新文章，有限回补进入阅读库并标为历史；日报仍按原始发布时间和现有新鲜度规则选择。未知发布日期不改成“今日”。成功空列表与请求失败不能都当“没有更新”。
- 公共文章去重可减少下载/提取，但个人是否看过、是否纳入日报和分析结果独立。文章摘要/引文需要可追溯原链接、公众号、发布日期、抓取时间和提取质量；正文不可用时只做基于实际取得证据的摘要。

## 推荐分期与验收门槛

**一期：小范围可用。** 做微信文章链接导入，以及用户明确提供/运营侧核验的无会话 RSS。先选 3–5 个目标公众号，验证源身份、普通请求可读、正文/日期、至少一周更新延迟与漏文情况；无可用源时提供单篇导入。建设源级状态与上述权限结构后再把订阅补充到个人日报。无需购买 Wechat2RSS。

**二期：私有连接器试验。** 一名愿意自行管理实例的测试用户，接 WeRSS 的只读 RSS/API；先不托管微信会话。显式配置正文与留存策略，检验授权失效/撤销、临时链接失效、历史页边界、停止访问限制、租户越权与日志泄漏。其定位是可选高级能力，不计入默认“长期稳定”承诺。

**另行触发的合作路线。** 只有出现自有/合作号需求时，才核验企业账号权限和第三方平台资质，再考虑官方授权、token 刷新和撤销处理；不作为上述两阶段的前置条件。对方没有授权的号仍回到公开源或链接导入，不能借合作账号扩大权限。

尚待验证：实际目标公众号名单与可用公开源；供应方对 Memhot 云端使用的具体许可；官方接口对群发/后台发布/旧文章的覆盖；WeRSS 实际微信会话寿命、各模式回补范围及版本默认值；微信文章删除/编辑/迁移后的身份归并；每源真实延迟与每月资源成本。当前证据足以筛选方案，尚不足以承诺全量或无维护订阅。
