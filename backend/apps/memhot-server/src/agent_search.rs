//! Optional account search providers enrich private research; public editions never
//! borrow a subscriber's credentials. Pi's free search remains the fallback.
use super::Scope;
use crate::{discovery::Material, error::Result, App};

pub(super) async fn run(
    app: &App,
    scope: &Scope,
    query: &str,
    recent: bool,
) -> Result<(Vec<Material>, Vec<String>)> {
    match scope {
        Scope::Public { catalog_id, .. } => Ok((
            crate::shared_public::agent_search(app, catalog_id, query).await?,
            vec![],
        )),
        Scope::Private { user, .. } => {
            let profile = app.store.profile(user)?;
            if !crate::search_providers::paid_configured(&profile["search"]) {
                return Ok((crate::agent_web::search(query).await?, vec![]));
            }
            let mut result =
                crate::search_providers::configured_search(&profile["search"], query, recent).await;
            scope.check(app)?;
            if result.needs_public_fallback {
                match crate::agent_web::search(query).await {
                    Ok(materials) => result.materials.extend(materials),
                    Err(error) if result.materials.is_empty() => return Err(error),
                    Err(_) => result
                        .warnings
                        .push("公开搜索暂不可用，可继续核对已有来源".into()),
                }
            }
            let mut seen = std::collections::HashSet::new();
            result
                .materials
                .retain(|m| seen.insert(crate::freshness::canonical(&m.url)));
            Ok((result.materials, result.warnings))
        }
    }
}
