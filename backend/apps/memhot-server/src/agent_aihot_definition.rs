//! Describe only the arguments that the authority layer accepts for each scope.
use super::Scope;
use serde_json::{json, Value};

pub(super) fn definition(scope: &Scope) -> Value {
    let robotics = matches!(scope, Scope::Public { catalog_id, .. } if catalog_id == "robotics");
    let actions = if robotics {
        json!(["search"])
    } else {
        json!(["latest", "search", "hot", "story", "daily", "weekly", "monthly"])
    };
    let mut properties = json!({"action":{"type":"string","enum":actions}});
    // A misleading optional storyId caused repeated rejected search calls in
    // real robotics QA. This scope has no story operation or arbitrary query.
    if !robotics {
        properties["storyId"] = json!({"type":"string","minLength":1,"maxLength":128,
            "description":"Only for action story; copy an ID from this run's hot results. Never send for search or other actions."});
    }
    if matches!(scope, Scope::Private { .. }) {
        properties["query"] = json!({"type":"string","minLength":2,"maxLength":180,
            "description":"Only for action search; public topic without personal or private text."});
        properties["period"] = json!({"type":"string","minLength":7,"maxLength":10,
            "description":"Only for daily, weekly, or monthly."});
    }
    let description = if robotics {
        "Discover robotics evidence using AIHOT's read-only MCP. Call exactly {\"action\":\"search\"}: the host supplies the fixed robotics query. Do not send query, storyId, or period. Use the search tool for other focused queries. AIHOT summaries are discoveries: use read_source before citing facts and preserve original links."
    } else {
        "Discover AI news using AIHOT's read-only MCP. Prefer daily for an edited overview, latest for recent picks, hot then story for a discovered event, search for public topics. Public requests use fixed catalog queries only. storyId is only for story and must come from this run's hot results. Private search queries must omit personal/private text. Preserve attribution and original links; summaries are discoveries, use read_source before citing facts."
    };
    json!({"name":"aihot","description":description,"inputSchema":{"type":"object",
        "properties":properties,"required":["action"],"additionalProperties":false}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robotics_exposes_only_the_fixed_search_accepted_by_host() {
        let scope = Scope::public("schema-test", "robotics").unwrap();
        let schema = definition(&scope)["inputSchema"].clone();
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        assert_eq!(schema["properties"]["action"]["enum"], json!(["search"]));
        assert_eq!(schema["additionalProperties"], false);
        let request =
            crate::aihot::request(&scope, &json!({"action":"search"}), &json!([])).unwrap();
        assert_eq!(request.args["q"], "robotics embodied intelligence");
        for field in ["query", "storyId", "period"] {
            let mut args = json!({"action":"search"});
            args[field] = json!("robotics");
            assert!(crate::aihot::request(&scope, &args, &json!([])).is_err());
        }
    }

    #[test]
    fn ai_and_private_keep_their_distinct_capabilities() {
        let scope = Scope::public("schema-test", "ai").unwrap();
        let properties = definition(&scope)["inputSchema"]["properties"].clone();
        assert!(properties.get("storyId").is_some());
        assert!(properties.get("query").is_none());
        assert!(properties.get("period").is_none());
        let scope = Scope::Private {
            user: "u".into(),
            job: None,
            epoch: 0,
            run_id: "schema-test".into(),
            references: vec![],
        };
        let properties = definition(&scope)["inputSchema"]["properties"].clone();
        for field in ["storyId", "query", "period"] {
            assert!(properties.get(field).is_some());
        }
    }
}
