//! Subscription search is local and tenant-scoped; queries never leave the server.
use super::Scope;
use crate::{
    error::{Error, Result},
    App,
};
use serde_json::{json, Value};

pub(super) fn definition() -> Value {
    json!({"name":"search_subscriptions","description":"Search the user's already collected subscriptions for evidence and reading leads. This tool searches only this user's enabled subscriptions, makes no external request, and costs no search-provider call. Follow subscriptionNextOffset with the same query to discover the rest of this user's full cache. Use list_candidates to inspect the retained candidates by pages. Summaries are not full articles; use read_source/open_page to verify originals. Keep source dates and distinguish historical background from today's news.","inputSchema":{"type":"object","properties":{"query":{"type":"string","minLength":2,"maxLength":180},"offset":{"type":"integer","minimum":0}},"required":["query"],"additionalProperties":false}})
}
pub(super) fn execute(app: &App, scope: &Scope, purpose: &str, args: &Value) -> Result<Value> {
    let Scope::Private { user, .. } = scope else {
        return Err(Error::bad("公共日报不能检索个人订阅"));
    };
    scope.check(app)?;
    let query = args["query"]
        .as_str()
        .filter(|q| (2..=180).contains(&q.trim().chars().count()))
        .ok_or_else(|| Error::bad("请输入2至180字的订阅查询词"))?;
    let offset = match args.get("offset") {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| Error::bad("订阅分页位置无效"))?,
    };
    let (found, more) = crate::subscription_items::search_materials_page(app, user, query, offset)?;
    let mut result = super::candidates::store_and_page(app, scope, purpose, &found)?;
    result["subscriptionNextOffset"] = if more {
        json!(offset + found.len())
    } else {
        Value::Null
    };
    result["scope"] = json!("personal_subscriptions");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_agent_never_advertises_personal_subscription_search() {
        let scope = Scope::Public {
            edition_key: "test".into(),
            catalog_id: "ai".into(),
        };
        let defs = crate::agent::tools::definitions(&scope, false);
        assert!(!defs
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "search_subscriptions"));
    }
}
