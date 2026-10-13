//! One numbering map for research links and the final published source list.
use crate::research_claims::Claim;
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) struct Citations {
    indices: Vec<usize>,
}
impl Citations {
    pub(crate) fn new(claims: &[Claim]) -> Self {
        Self {
            indices: claims
                .iter()
                .map(|claim| claim.evidence)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
    pub(crate) fn sources(&self, evidence: &[Value]) -> Vec<Value> {
        self.indices
            .iter()
            .map(|&index| evidence[index].clone())
            .collect()
    }
    pub(crate) fn reference(&self, claim: &Claim, evidence: &[Value]) -> String {
        let number = self
            .indices
            .binary_search(&claim.evidence)
            .expect("reviewed claim has a published source")
            + 1;
        let url = evidence[claim.evidence]["url"]
            .as_str()
            .unwrap_or_default()
            .replace(')', "%29");
        format!("[{number}]({url})")
    }
}
