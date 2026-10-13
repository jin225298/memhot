//! Paper calls have a durable pre-dispatch fence: uncertainty never causes quota replay.
use super::{state, Scope};
use crate::{
    alphaxiv, clip,
    discovery::Material,
    error::{Error, Result},
    App,
};
use serde_json::{json, Value};
fn paper_index(materials: &[Material], url: &str) -> Option<usize> {
    materials
        .iter()
        .position(|m| alphaxiv::paper_url(&m.url).is_ok_and(|canonical| canonical == url))
}
pub(super) async fn execute(
    app: &App,
    scope: &Scope,
    purpose: &str,
    saved: &mut state::Saved,
    args: &Value,
) -> Result<Value> {
    if !alphaxiv::available(app, scope) {
        return Err(Error::missing());
    }
    let request = alphaxiv::request(args)?;
    let mut materials = saved.materials();
    let key = crate::hash(request.start().to_string().as_bytes());
    if !saved.context["paperCalls"].is_object() {
        saved.context["paperCalls"] = json!({})
    }
    let value = if let Some(receipt) = saved.context["paperCalls"].get(&key) {
        if receipt["status"] != "complete" {
            return Err(Error::upstream(
                "upstream",
                "这次论文请求的结果不确定，已阻止自动重复调用，请使用其他来源",
            ));
        }
        receipt["value"].clone()
    } else {
        let calls = saved.context["paperCalls"]
            .as_object()
            .ok_or_else(Error::internal)?;
        let limit = match request.tool {
            "discover_papers" => 2,
            "list_library" => 1,
            _ => 8,
        };
        if calls.values().filter(|v| v["tool"] == request.tool).count() >= limit {
            return Err(Error::upstream(
                "rate_limited",
                "本轮论文调用已达上限，请使用已有材料",
            ));
        }
        claim(app, scope, purpose, &key, request.tool, limit)?;
        saved.context["paperCalls"][&key] = json!({"status":"pending","tool":request.tool});
        state::persist(app, scope, purpose, saved)?;
        let value = alphaxiv::fetch(app, scope, &request).await?;
        saved.context["paperCalls"][&key] =
            json!({"status":"complete","tool":request.tool,"value":value});
        state::persist(app, scope, purpose, saved)?;
        value
    };
    scope.check(app)?;
    if args["action"] == "read" {
        let url = alphaxiv::paper_url(value["url"].as_str().unwrap_or(""))?;
        let text = value["text"]
            .as_str()
            .filter(|s| s.len() <= 400000 && s.chars().count() >= 200)
            .ok_or_else(|| Error::upstream("upstream", "未取得论文原文"))?;
        if value["fulltext"] != true {
            return Err(Error::upstream("upstream", "论文响应没有原文标记"));
        }
        let index = paper_index(&materials, &url);
        let mut material = index.map(|i| materials[i].clone()).unwrap_or(Material {
            title: clip(text.lines().next().unwrap_or("论文"), 200),
            url: url.clone(),
            text: String::new(),
            published: None,
            discovered: Some(crate::now()),
            discovered_via: Some("https://www.alphaxiv.org/".into()),
            publication_source: "unknown",
            publisher: "arXiv · alphaXiv 全文".into(),
            fulltext: false,
            pdf: None,
        });
        material.text = text.into();
        material.fulltext = true;
        // alphaXiv supplies its own text range; a replaced PDF's page counts no longer apply.
        material.pdf = None;
        let mut response = super::tools::source(&material);
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let total = text.chars().count();
        response["text"] = json!(text.chars().skip(offset).take(18000).collect::<String>());
        response["nextOffset"] = if offset + 18000 < total {
            json!(offset + 18000)
        } else {
            Value::Null
        };
        response["truncated"] = value["truncated"].clone();
        response["warning"] =
            json!("原论文提取文本。尚未读取的段落不可视为已核对；研究结论不等于已被独立验证。");
        if let Some(i) = index {
            materials[i] = material
        } else {
            materials.push(material)
        }
        saved.set_materials(&materials);
        return Ok(response);
    }
    let found = value["sources"]
        .as_array()
        .ok_or_else(|| Error::upstream("upstream", "论文列表格式无效"))?;
    let mut discoveries = vec![];
    for v in found.iter() {
        let Ok(url) = alphaxiv::paper_url(v["url"].as_str().unwrap_or("")) else {
            continue;
        };
        let published = v["publishedAt"]
            .as_str()
            .filter(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok())
            .map(str::to_owned);
        let material = Material {
            title: clip(v["title"].as_str().unwrap_or("论文"), 240),
            url,
            text: clip(v["text"].as_str().unwrap_or(""), 1400),
            published,
            discovered: Some(crate::now()),
            discovered_via: Some("https://www.alphaxiv.org/".into()),
            publication_source: "search_reported_publication",
            publisher: "arXiv · alphaXiv 检索".into(),
            fulltext: false,
            pdf: None,
        };
        discoveries.push(material);
    }
    let mut result = super::candidates::store_and_page(app, scope, purpose, &discoveries)?;
    result["folders"] = value["folders"].clone();
    result["warning"] = value["warning"].clone();
    result["private"] = json!(true);
    Ok(result)
}

#[cfg(test)]
#[path = "agent_papers_tests.rs"]
mod tests;

// Only hashes and operation names survive the 24h raw-context retention window.
// This receipt is not a new content cache; expired/missing results never authorize replay.
fn claim(
    app: &App,
    scope: &Scope,
    purpose: &str,
    key: &str,
    tool: &str,
    limit: usize,
) -> Result<()> {
    let Scope::Private { user, epoch, .. } = scope else {
        return Err(Error::missing());
    };
    let run = scope.key(purpose);
    let id = crate::hash(format!("{run}:{key}").as_bytes());
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current: i64 = tx.query_row("SELECT epoch FROM users WHERE id=?", [user], |r| r.get(0))?;
    if current != *epoch {
        return Err(Error::upstream("context_deleted", "研究内容已删除"));
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='paper_receipt' AND id=?)",
        rusqlite::params![user, id],
        |r| r.get(0),
    )?;
    if exists {
        return Err(Error::upstream(
            "upstream",
            "之前的论文调用已发出，结果已过期或尚未确认，已阻止自动重复调用",
        ));
    }
    let count:i64=tx.query_row("SELECT count(*) FROM records WHERE user_id=? AND kind='paper_receipt' AND json_extract(data,'$.run')=? AND json_extract(data,'$.tool')=?",rusqlite::params![user,run,tool],|r|r.get(0))?;
    if count >= limit as i64 {
        return Err(Error::upstream("rate_limited", "本轮论文调用已达上限"));
    }
    tx.execute(
        "INSERT INTO records(user_id,kind,id,data,created) VALUES(?,'paper_receipt',?,?,?)",
        rusqlite::params![
            user,
            id,
            json!({"run":run,"tool":tool}).to_string(),
            crate::now()
        ],
    )?;
    tx.commit()?;
    Ok(())
}
