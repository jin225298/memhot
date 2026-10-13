# 授权阅读接口

所有路径均位于 `/api/v1`，复用现有 Bearer 设备会话。报告必须属于当前用户且已经交付；跨用户报告、未交付报告及不属于该报告的事件/证据均返回 404。公共成刊的 ID 和 URL 不能替代个人报告授权。

## 历史与收藏

`GET /reading?kind=research|digest&period=all|week|month&saved=false&q=&category=&limit=20&cursor=`

- `kind` 可省略；`saved=true` 仅看收藏，`false` 或省略表示全部。
- `week`、`month` 分别为过去 7、30 天；交付时间优先取 `availableAt`，否则取 `createdAt`。
- `q` 对授权报告的标题、概览、正文、事件描述、来源标题及保留引文做不区分大小写的子串搜索，最多 200 字。不会搜索其他用户、未授权公共成刊或任意 URL。
- `category` 匹配事件的编辑分类，最多 80 字；`limit` 为 1–50，默认 20。游标为不透明字符串，与用户和筛选条件绑定；筛选变化后须重新从第一页开始。
- 返回 `{reports, nextCursor?, overview:{title,summary,reportCount,storyCount}, categories:string[]}`。摘要保留报告/事件元数据，增加 `saved:boolean` 和 `sourceCount`，正文和来源正文不随列表返回。
- `overview` 统计整个筛选结果中的已交付报告数、事件/快讯数和近期真实标题。未完成的旧摘录保留在历史列表，但不计入已交付数和近期标题，概览另行提示其数量。它由确定性代码汇总，不调用模型。分类列表应用类型、时间、收藏和搜索条件，但不应用当前分类条件。
- 缺少所有事件与快讯的旧日报，或存在显式空 `claims` 数组的旧研究报告，标记为 `status:"incomplete"`，仍可从历史找到。已有快讯的成刊不会因此标记为未完成。
- 历史不受首页 60 条限制。服务先在 SQL 筛选授权引用，再逐份扫描正文；内存中仅保存报告 ID、聚合信息和当前页摘要。

`PUT /reports/{id}/saved {"saved":true|false}` 返回 `{id,saved}`。同一状态重复提交幂等；收藏不会延长原文缓存的保留期。报告删除后收藏不再可见。

## 事件与材料

`GET /reports/{id}/events/{storyId}` 返回 `{story, related:[{reportId,storyId,title,summary,date,relationship}]}`，最多 20 条已授权相关记录。只有非空 `eventKey` 相同或双方事件确实引用同一来源 URL 才标记 `same_event`；同类别、同主题但没有事件身份依据的条目标记 `related`。

`GET /reports/{id}/materials/{evidenceId}` 返回 `{evidenceId,title,url,publisher,publishedAt?,text,retrievalKind,availability,truncated,message?,translation?}`。

- 只读现有公共页面缓存或本用户的私有原文缓存，不在阅读时联网抓取。
- `retrievalKind` 为 `page|metadata_or_abstract`，`availability` 为 `full|excerpt`。论文摘要/元数据不会伪称已读全文。
- 原文最长保留 24 小时。即使清理器尚未执行，过期缓存也立即回退到报告保存的引文，标记为摘要。
- 展示/翻译上限为 12000 字；截段有 `truncated:true` 和明确提示。私有旧抓取达到其 18000 字存储上限时也诚实标注已截段。

## 按需翻译

`POST /reports/{id}/materials/{evidenceId}/translation` 返回 HTTP 202 `{id,jobId,status,message?}`。不接受 URL、原文或用户指定的模型调用参数。翻译经统一 Agent SDK 的无工具执行路径进行，沿用个人调用账本和额度，排队本身不预先扣费。

幂等键包含用户账号版本、授权报告版本、证据版本、可用材料内容、截段状态及原文保留期限。相同输入重复提交复用任务/已完成译文，即使额度已经耗尽也不会重复收费。报告或证据变化后可创建新的逻辑任务，失败任务通过现有任务重试接口恢复；未知计费结果仍需原有的显式确认。

读取材料时，`translation` 包含 `status`、`jobId`，完成时还包含 `text`、`availability` 和 `truncated`。状态为 `queued|running|ready|failed|blocked|unavailable`；失败提示为服务器固定文案，不回显供应商原始错误内容。无可翻译文字时返回 409 `material_text_unavailable`；待处理任务达到 8 个时返回 429 `translation_queue_full`。

模型若原样回显非中文英文段落（忽略空白差异），返回 `translation_unchanged` 并保留失败任务，不把原文发布为成功译文。已有中文或仅含短专名的材料不因此被误拒绝；这项校验不替代翻译质量审查。

翻译前及最终提交时检查原文与授权上下文。最终事务同时校验报告仍存在、账号 epoch、报告/证据/缓存内容及任务租约；删除、修改、缓存过期或旧执行租约丢失均不能写回旧译文。全文译文与原文同时到期，读取端立即隐藏，清理器随后移除；保留引文的翻译可随报告保存。
