use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub key: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub source: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogRequest {
    pub catalog_id: String,
    #[serde(default)]
    pub skip: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

pub type AnimePreview = Value;
pub type AnimeMetadata = Value;
