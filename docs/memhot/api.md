# Memhot 0.2 · API v1 / 客户端协作契约

基址 https://memhot.angelkang.com。JSON camelCase。私有接口Authorization: Bearer device-token。错误 `{error:{code,message,requestId?}}`；不泄露内部异常/秘密。所有列表有上限。来源/报告/对话ID服务端生成，客户端不得指定userId。

- `GET /healthz` → `{status:"ok",version}`（不含配置）。
- 邀请期客户端使用 `POST /api/v1/auth/register`、`POST /api/v1/auth/login`、`POST /api/v1/auth/reset`；账户/测试充值契约见 [accounts-and-test-credits.md](accounts-and-test-credits.md)。
- `POST /api/v1/pair` `{code,deviceName}` → `{token,user:{id,name}}`；旧客户端的稳定接口，仅供既有会话迁移；新应用不展示配对入口。
- `GET /api/v1/home` → `{profile,thoughts,reports,sources,jobs,conversations}`；profile不含秘密；来源不含原文、报告列表不含正文/完整证据而附sourceCount；conversations仅包含id、reportId、thoughtId、updatedAt元数据，读取消息需单独调用。
- `GET /api/v1/delivery` → `{reports:[{id,createdAt}]}`，最新60条报告的轻量通知清单。
- `GET /api/v1/profile` → profile。
- `PUT /api/v1/profile` `{name,interests:string[],timezone,digestHour,researchHour,retentionDays,models?,feeds?}` → profile；patch语义保留省略字段。
- 托管邀请版使用服务端运营配置，普通客户端不提供模型密钥设置。独立部署协议中profile.models输入 `{text:{protocol,baseUrl,model,apiKey?},asr:{protocol,baseUrl,model,apiKey?}}`，输出只保留公开字段及`configured:boolean`；protocol文本`openai|anthropic|gemini`，ASR `openai|qwen|mimo`。模型为空为未配置，不能用默认假内容代替。
- `POST /api/v1/models/test` `{kind:"text"|"asr"}` → `{ok,message}`，只测试已保存配置，不返回密钥。
- `POST /api/v1/sources` `{title,text,kind:"note"|"import",speakerMode:"roles"|"self"|"unknown",occurredAt?,clientId?}` → Source；相同clientId幂等。文本上限200000 UTF-8字节。导入默认roles：支持user/assistant文本、messages JSON、ChatGPT mapping；无可信角色时保留原文并标记roles_required。self用于明确本人自述；unknown用于多角色或身份未确认的会议/文本，原始标签保留但不当作用户观点。音频说话人标记unknown。
- `POST /api/v1/audio`：`Content-Type:audio/wav`，`Idempotency-Key:chunk-id`，`X-Captured-At:ISO8601`，body WAV；→202在文字或无语音结果耐久提交后返回；音频只在请求内存中处理。同一用户+chunkID返回原结果，不再次计费。未知付费结果需明确重试，不能假定失败即未收费。
- `GET /api/v1/sources/:id` → Source；`DELETE` →204，同时撤销依赖的个性化输入。
- `POST /api/v1/research` `{thoughtId?,kind?:"research"|"digest"}` →202，手动更新，复用已排队/运行/待额度任务；研究默认覆盖全部符合条件的新/变更话题，日报按各兴趣拆成任务。
- `POST /api/v1/jobs/:id/retry` `{acceptDuplicateCharge:boolean}` →202新任务；仅本用户failed任务，幂等返回同一重建任务。结果不明或已收费但未完整返回时需true明确确认，否则409。原音频/上下文过期不可重试。
- `GET /api/v1/reports/:id` → Report；`DELETE` →204。
- `POST /api/v1/thoughts/:id/feedback` `{action:"keep"|"dismiss"|"resolved",note?}` → Thought。
- `POST /api/v1/discuss` `{reportId?,thoughtId?,conversationId?,message}` → `{conversationId,message:Message}`，可先非流式，取消/超时在UI明确表达。
- `GET /api/v1/conversations/:id` → `{id,messages:Message[]}`。
- `DELETE /api/v1/device` →204，撤销当前bearer token；其他设备不受影响。
- `DELETE /api/v1/account/data` →204，删除本用户资料、派生内容、待执行任务；token保留，设置重置。

数据形状：

`Profile={name,interests,timezone,digestHour,researchHour,retentionDays,models?,feeds?:string[]}`

`Source={id,title,kind,text?,createdAt,status:"queued"|"transcribing"|"ready"|"failed",analysisStatus?,analysisMessage?,analysisCoverage?:{includedChars,totalChars,complete},error?}`

`Thought={id,title,question,quote,sourceId?,assumptions:string[],gaps:string[],status:"open"|"kept"|"dismissed"|"resolved",createdAt,analysisStatus?,origin?,researchEligible?}`

`Report={id,title,question?,kind:"research"|"digest",summary,body,thoughtId?,createdAt,status:"ready"|"partial",sources:Evidence[],gaps:string[]}`

`Evidence={id,title,url,publisher?,publishedAt?,retrievedAt,excerpt,stance:"support"|"counter"|"context",citations?:{claim,quote,stance,type}[],retrievalKind?}`

`Message={id,role:"user"|"assistant",text,createdAt,sources?:Evidence[]}`

讨论 `Message.sources` 仅包含回答中实际引用的已授权候选 URL；没有引用时为空，不把未使用的补充检索候选展示为支持材料。历史会话读取使用相同过滤。

`Job={id,kind,status:"queued"|"running"|"completed"|"failed",message?,createdAt,retryable?:boolean,requiresChargeConfirmation?:boolean}`

## 原生录音桥（Tauri plugin memhot-capture）

JavaScript invoke命令：`plugin:memhot-capture|configure` args `{baseUrl,token,userId}`；`get_config` → `{baseUrl,token,userId}`仅供受信任应用UI内存使用；`start`、`stop`、`status`、`open_battery_settings`、`request_notifications`；`clear_pending` → `{clearedCount}`。清理前停止录音并等待本地在途上传结束，仅删除当前账号队列；账号清理的调用顺序为clear_pending成功后DELETE account/data。应用后台上传独立于WebView。

`status` → `{supported:boolean,state:"idle"|"recording"|"paused"|"error",queuedCount:number,uploadedCount:number,error?:string,notificationsAllowed:boolean,notificationGuide:string}`。

移动端记录本地队列并对上述audio端点上传；收到202表示文字/无语音结果已耐久提交，设备可删已确认片段。服务端不持久保留新音频，不能依靠云端文件重试。权限拒绝明确返回错误，不创建假录音状态。非Android先报告supported:false并保留导入/阅读/讨论功能，桌面录音在后续验证中接入既有采集模块。

队列归属由服务器地址和稳定userId决定；更换device token仍可恢复同账号录音，切换账号不可混传。空token表示断开并停止后台日报同步，保留已归属的本地待上传片段。日报同步由Android WorkManager按30分钟周期在联网时检查；系统Doze/OEM限制可延后，不能承诺准点通知。


报告标题使用短思考标题，完整研究问题保留于 `question`。每条通过复核的引文保存在 `Evidence.citations`，同来源多结论不会覆盖；`excerpt` 为不同已核对引文的组合，混合立场来源顶层 `stance` 为 `context`，具体立场以每条 citation 为准。范围限制、缺少证据和推断不能单独满足研究 `ready` 所需的实证反证条件。

主来源分析最多读取30000字符，相邻来源各1500字符；`analysisCoverage` 表示本次主来源实际提供给模型的范围，超过上限时 `complete:false` 并有 `analysisMessage`，完整原文仍保留。它表示输入覆盖范围，不保证模型发现全部议题。


## 0.2 多话题、认知与引导

- `GET /api/v1/thoughts?limit=50&cursor=…` 分页读取全部话题，`POST /api/v1/thoughts`创建；`PUT/DELETE /api/v1/thoughts/:id`使用租户与版本边界。首页的有界摘要不能代替完整话题列表。
- `POST /api/v1/sources/:id/analyze`安排重新整理，事务清除旧覆盖。`analysisCoverage`含`scannedChars`、`includedChars`、`totalChars`、`complete`和退回数量；扫描完不代表所有窗口输出均已核验。
- `GET /api/v1/daily-summaries`返回分日期、分话题的整理，使用用户时区。
- `GET/POST /api/v1/cognition`、`PUT/DELETE /api/v1/cognition/:id`管理个人理解；类型为interest/preference/goal/belief/constraint，status为active/pending/dismissed，区分manual/explicit/inferred，更新携带revision。未知说话人只能产生pending候选。
- `GET /api/v1/preferences/setup`返回`{completed,interests,reading,thinking}`；前三组选项由服务端固定目录定义。
- `POST /api/v1/preferences/setup`输入`{interestIds?,customInterests?,readingIds?,thinkingIds?,skip?}`。事务合并最多20个兴趣与手动偏好；重复已完成请求不覆盖后续编辑，未知字段和选项拒绝。skip只记录完成，不编造偏好。接口始终需要认证。
- `POST /api/v1/audio/transcript`接收本机转写文字，受相同租户/幂等边界保护；本机模型失败不默默转为音频上传。
- 原生新增`download_local_model`、`cancel_local_model_download`、`remove_local_model`、`set_asr_mode`、`retry_pending`、`get_launch_target`。`status`补充asrMode、模型状态、blockedCount与原生单调时钟elapsedMs。

公开领域日报正文只存公共缓存一次；`/reports/:id`仍按用户投递引用鉴权，不能以公共edition ID绕过账号边界。
