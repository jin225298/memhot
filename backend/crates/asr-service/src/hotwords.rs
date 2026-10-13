//! 热词管理模块
//!
//! 为 ASR 识别提供热词支持,提升专有名词、产品名称等关键词的识别准确率。
//! 热词权重范围 [-6, 5]: 正值增加识别概率,负值减小识别概率。

use crate::{AsrError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 热词权重有效范围
const MIN_WEIGHT: i32 = -6;
const MAX_WEIGHT: i32 = 5;
/// 默认权重(阿里云推荐值)
const DEFAULT_WEIGHT: i32 = 2;
/// 最大热词数量(阿里云限制)
const MAX_HOTWORDS: usize = 500;

/// 单个热词条目
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotwordEntry {
    /// 热词文本(最长 10 字符)
    pub word: String,
    /// 权重 [-6, 5]
    pub weight: i32,
    /// 备注说明(可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 热词词表
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotwordList {
    /// 词表名称
    pub name: String,
    /// 热词条目
    pub entries: Vec<HotwordEntry>,
    /// 词表备注
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl HotwordList {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entries: Vec::new(),
            description: None,
        }
    }

    /// 添加热词,使用默认权重
    pub fn add_word(&mut self, word: impl Into<String>) -> Result<()> {
        self.add_word_with_weight(word, DEFAULT_WEIGHT)
    }

    /// 添加热词并指定权重
    pub fn add_word_with_weight(&mut self, word: impl Into<String>, weight: i32) -> Result<()> {
        let word = word.into();

        // 验证词长度
        if word.chars().count() > 10 {
            return Err(AsrError::UnsupportedFormat(format!(
                "热词「{}」超过 10 字符限制",
                word
            )));
        }

        // 验证权重范围
        if weight < MIN_WEIGHT || weight > MAX_WEIGHT {
            return Err(AsrError::UnsupportedFormat(format!(
                "权重 {} 超出范围 [{}, {}]",
                weight, MIN_WEIGHT, MAX_WEIGHT
            )));
        }

        // 检查数量限制
        if self.entries.len() >= MAX_HOTWORDS {
            return Err(AsrError::UnsupportedFormat(format!(
                "热词数量已达上限 {}",
                MAX_HOTWORDS
            )));
        }

        self.entries.push(HotwordEntry {
            word,
            weight,
            note: None,
        });

        Ok(())
    }

    /// 转换为阿里云 API 所需的 Map 格式 {"word": weight}
    pub fn to_word_weights(&self) -> HashMap<String, i32> {
        self.entries
            .iter()
            .map(|entry| (entry.word.clone(), entry.weight))
            .collect()
    }

    /// 从 JSON 字符串加载
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| AsrError::UnsupportedFormat(format!("热词 JSON 解析失败: {}", e)))
    }

    /// 导出为 JSON 字符串
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| AsrError::UnsupportedFormat(format!("热词 JSON 序列化失败: {}", e)))
    }

    /// 验证词表有效性
    pub fn validate(&self) -> Result<()> {
        if self.entries.len() > MAX_HOTWORDS {
            return Err(AsrError::UnsupportedFormat(format!(
                "热词数量 {} 超过限制 {}",
                self.entries.len(),
                MAX_HOTWORDS
            )));
        }

        for (idx, entry) in self.entries.iter().enumerate() {
            if entry.word.is_empty() {
                return Err(AsrError::UnsupportedFormat(format!(
                    "第 {} 个热词为空",
                    idx + 1
                )));
            }
            if entry.word.chars().count() > 10 {
                return Err(AsrError::UnsupportedFormat(format!(
                    "第 {} 个热词「{}」超过 10 字符",
                    idx + 1,
                    entry.word
                )));
            }
            if entry.weight < MIN_WEIGHT || entry.weight > MAX_WEIGHT {
                return Err(AsrError::UnsupportedFormat(format!(
                    "第 {} 个热词权重 {} 超出范围",
                    idx + 1,
                    entry.weight
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_word_with_default_weight() {
        let mut list = HotwordList::new("test");
        list.add_word("时刻").unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].word, "时刻");
        assert_eq!(list.entries[0].weight, DEFAULT_WEIGHT);
    }

    #[test]
    fn reject_word_exceeding_length() {
        let mut list = HotwordList::new("test");
        let result = list.add_word("这是一个超过十个字符的热词测试");
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_weight() {
        let mut list = HotwordList::new("test");
        assert!(list.add_word_with_weight("test", 10).is_err());
        assert!(list.add_word_with_weight("test", -10).is_err());
    }

    #[test]
    fn convert_to_word_weights_map() {
        let mut list = HotwordList::new("test");
        list.add_word_with_weight("Qwen3-ASR", 3).unwrap();
        list.add_word_with_weight("时刻", 2).unwrap();

        let map = list.to_word_weights();
        assert_eq!(map.get("Qwen3-ASR"), Some(&3));
        assert_eq!(map.get("时刻"), Some(&2));
    }

    #[test]
    fn json_roundtrip() {
        let mut list = HotwordList::new("产品术语");
        list.description = Some("常用产品名称".into());
        list.add_word_with_weight("Qwen3-ASR", 3).unwrap();

        let json = list.to_json().unwrap();
        let restored = HotwordList::from_json(&json).unwrap();

        assert_eq!(restored.name, list.name);
        assert_eq!(restored.entries, list.entries);
        assert_eq!(restored.description, list.description);
    }
}
