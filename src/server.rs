use crate::centralization::{
    centralize_source_previews, combine_anime_seasons, decode_refs, fill_missing_episode_air_dates,
    fill_missing_episode_thumbnails, has_episode_rows, metadata_queries, metadata_title_keys,
    normalize_title_key, string_field,
};
use crate::models::{
    AnimeMetadata, AnimePreview, CatalogRequest, SearchRequest, Source, SourceRef,
};
use crate::source_client::UpstreamClient;
use crate::tvmaze::collect_tvmaze_episode_image_sources;
use anyhow::{anyhow, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

const DEFAULT_SOURCES: &[&str] = &[
    "http://127.0.0.1:8787",
    "http://127.0.0.1:8788",
    "http://127.0.0.1:8789",
];

#[derive(Clone)]
struct AppState {
    sources: Vec<Source>,
    client: UpstreamClient,
}

pub async fn serve() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let state = AppState {
        sources: configured_sources(),
        client: UpstreamClient::new()?,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/catalog", post(catalog))
        .route("/search", post(search))
        .route("/anime/:id", get(anime))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8790);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Typenx addon listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "message": null}))
}

async fn manifest() -> Json<Value> {
    Json(manifest_payload())
}

async fn catalog(
    State(state): State<AppState>,
    Json(request): Json<CatalogRequest>,
) -> Result<Json<Value>, AppError> {
    let mut items = vec![];
    for source in &state.sources {
        let response = state
            .client
            .post_json(
                &source.base_url,
                "catalog",
                json!({
                    "catalog_id": request.catalog_id,
                    "skip": request.skip,
                    "limit": request.limit,
                    "query": request.query,
                }),
            )
            .await;
        collect_previews(&mut items, source, response);
    }
    Ok(Json(json!({"items": centralize_source_previews(&items)})))
}

async fn search(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Value>, AppError> {
    let mut items = vec![];
    for source in &state.sources {
        let response = state
            .client
            .post_json(
                &source.base_url,
                "search",
                json!({
                    "query": request.query,
                    "limit": request.limit,
                }),
            )
            .await;
        collect_previews(&mut items, source, response);
    }
    Ok(Json(json!({"items": centralize_source_previews(&items)})))
}

async fn anime(
    State(state): State<AppState>,
    Path(anime_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let mut refs = decode_refs(&anime_id)?;
    refs = expand_season_refs(&state.client, &state.sources, &refs).await;
    let mut seasons = fetch_seasons(&state.client, &state.sources, &refs).await?;
    let original_seasons = seasons.clone();
    if !has_episode_rows(&seasons) {
        let fallback_refs =
            expand_episode_fallback_refs(&state.client, &state.sources, &refs, &seasons).await;
        let fallback_seasons = fetch_seasons(&state.client, &state.sources, &fallback_refs).await?;
        if has_episode_rows(&fallback_seasons) {
            refs = fallback_refs;
            seasons = fallback_seasons;
        }
    }
    let mut thumbnail_sources = seasons.clone();
    thumbnail_sources.extend(original_seasons.clone());
    thumbnail_sources.extend(
        collect_thumbnail_backup_seasons(&state.client, &state.sources, &refs, &thumbnail_sources)
            .await,
    );
    thumbnail_sources
        .extend(collect_tvmaze_episode_image_sources(&state.client, &thumbnail_sources).await);
    let combined = combine_anime_seasons(&seasons)?;
    let combined = fill_missing_episode_air_dates(combined, &seasons);
    Ok(Json(fill_missing_episode_thumbnails(
        combined,
        &thumbnail_sources,
    )))
}

fn collect_previews(
    items: &mut Vec<(Source, AnimePreview)>,
    source: &Source,
    response: Result<Value>,
) {
    let Ok(response) = response else { return };
    let Some(response_items) = response.get("items").and_then(Value::as_array) else {
        return;
    };
    items.extend(
        response_items
            .iter()
            .cloned()
            .map(|item| (source.clone(), item)),
    );
}

async fn fetch_seasons(
    client: &UpstreamClient,
    sources: &[Source],
    refs: &[SourceRef],
) -> Result<Vec<AnimeMetadata>> {
    let mut seasons = vec![];
    for reference in refs {
        let source = source_by_key(sources, &reference.source)?;
        seasons.push(
            client
                .get_json(&source.base_url, &format!("anime/{}", reference.id))
                .await?,
        );
    }
    Ok(seasons)
}

async fn expand_season_refs(
    client: &UpstreamClient,
    sources: &[Source],
    refs: &[SourceRef],
) -> Vec<SourceRef> {
    let mut seasons = vec![];
    for reference in refs {
        let Ok(source) = source_by_key(sources, &reference.source) else {
            continue;
        };
        if let Ok(metadata) = client
            .get_json(&source.base_url, &format!("anime/{}", reference.id))
            .await
        {
            seasons.push((source.clone(), metadata));
        }
    }
    let mut title_keys_by_source: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();
    let mut queries_by_source: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();
    for (source, metadata) in &seasons {
        let season_slice = [metadata.clone()];
        title_keys_by_source
            .entry(source.key.clone())
            .or_default()
            .extend(metadata_title_keys(&season_slice));
        queries_by_source
            .entry(source.key.clone())
            .or_default()
            .extend(metadata_queries(&season_slice));
    }

    let mut expanded = refs.to_vec();
    let mut seen = refs
        .iter()
        .map(|reference| (reference.source.clone(), reference.id.clone()))
        .collect::<HashSet<_>>();
    for (source, _) in seasons {
        let source_keys = title_keys_by_source
            .get(&source.key)
            .cloned()
            .unwrap_or_default();
        let mut queries = queries_by_source
            .get(&source.key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        queries.sort();
        for query in queries {
            let Ok(response) = client
                .post_json(
                    &source.base_url,
                    "search",
                    json!({"query": query, "limit": 20}),
                )
                .await
            else {
                continue;
            };
            let Some(items) = response.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                let Some(title) = string_field(item, "title") else {
                    continue;
                };
                if !source_keys.contains(&normalize_title_key(title)) {
                    continue;
                }
                let Some(id) = string_field(item, "id") else {
                    continue;
                };
                if seen.insert((source.key.clone(), id.to_string())) {
                    expanded.push(SourceRef {
                        source: source.key.clone(),
                        id: id.to_string(),
                    });
                }
            }
        }
    }
    expanded
}

async fn expand_episode_fallback_refs(
    client: &UpstreamClient,
    sources: &[Source],
    refs: &[SourceRef],
    seasons: &[AnimeMetadata],
) -> Vec<SourceRef> {
    let used_sources = refs
        .iter()
        .map(|reference| reference.source.clone())
        .collect::<HashSet<_>>();
    let title_keys = metadata_title_keys(seasons);
    let mut queries = metadata_queries(seasons).into_iter().collect::<Vec<_>>();
    queries.sort();
    if title_keys.is_empty() || queries.is_empty() {
        return refs.to_vec();
    }
    let mut fallback_refs = vec![];
    for source in sources {
        if used_sources.contains(&source.key) {
            continue;
        }
        for query in &queries {
            let Ok(response) = client
                .post_json(
                    &source.base_url,
                    "search",
                    json!({"query": query, "limit": 20}),
                )
                .await
            else {
                continue;
            };
            let Some(items) = response.get("items").and_then(Value::as_array) else {
                continue;
            };
            if let Some(id) = items
                .iter()
                .find(|item| {
                    string_field(item, "title")
                        .is_some_and(|title| title_keys.contains(&normalize_title_key(title)))
                })
                .and_then(|item| string_field(item, "id"))
            {
                fallback_refs.push(SourceRef {
                    source: source.key.clone(),
                    id: id.to_string(),
                });
                break;
            }
        }
    }
    if fallback_refs.is_empty() {
        refs.to_vec()
    } else {
        fallback_refs
    }
}

async fn collect_thumbnail_backup_seasons(
    client: &UpstreamClient,
    sources: &[Source],
    refs: &[SourceRef],
    seasons: &[AnimeMetadata],
) -> Vec<AnimeMetadata> {
    let used_sources = refs
        .iter()
        .map(|reference| reference.source.clone())
        .collect::<HashSet<_>>();
    let title_keys = metadata_title_keys(seasons);
    let mut queries = metadata_queries(seasons).into_iter().collect::<Vec<_>>();
    queries.sort();
    if title_keys.is_empty() || queries.is_empty() {
        return vec![];
    }
    let mut backups = vec![];
    let mut seen = refs
        .iter()
        .map(|reference| (reference.source.clone(), reference.id.clone()))
        .collect::<HashSet<_>>();
    for source in sources {
        if used_sources.contains(&source.key) {
            continue;
        }
        for query in &queries {
            let Ok(response) = client
                .post_json(
                    &source.base_url,
                    "search",
                    json!({"query": query, "limit": 20}),
                )
                .await
            else {
                continue;
            };
            let Some(items) = response.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                let Some(title) = string_field(item, "title") else {
                    continue;
                };
                if !title_keys.contains(&normalize_title_key(title)) {
                    continue;
                }
                let Some(id) = string_field(item, "id") else {
                    continue;
                };
                if !seen.insert((source.key.clone(), id.to_string())) {
                    continue;
                }
                if let Ok(metadata) = client
                    .get_json(&source.base_url, &format!("anime/{id}"))
                    .await
                {
                    backups.push(metadata);
                }
            }
        }
    }
    backups
}

fn configured_sources() -> Vec<Source> {
    let raw = std::env::var("TYPENX_SEASON_SOURCES").unwrap_or_else(|_| DEFAULT_SOURCES.join(","));
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, url)| Source {
            key: format!("source-{}", index + 1),
            base_url: url.trim_end_matches('/').to_string(),
        })
        .collect()
}

fn source_by_key<'a>(sources: &'a [Source], key: &str) -> Result<&'a Source> {
    sources
        .iter()
        .find(|source| source.key == key)
        .ok_or_else(|| anyhow!("Unknown upstream source: {key}"))
}

pub fn manifest_payload() -> Value {
    json!({
        "id": "typenx-addon-season-centralizer",
        "name": "Season Centralizer",
        "version": "0.1.0",
        "description": "Combines split anime seasons from MAL, AniList, and Kitsu into one show.",
        "icon": null,
        "resources": ["catalog", "search", "anime_meta"],
        "catalogs": [
            {"id": "popular", "name": "Popular Anime", "content_type": "anime", "filters": []},
            {"id": "airing", "name": "Airing Anime", "content_type": "anime", "filters": []},
            {"id": "trending", "name": "Trending Anime", "content_type": "anime", "filters": []}
        ]
    })
}

struct AppError(anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": self.0.to_string()})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_exposes_required_routes() {
        let manifest = manifest_payload();
        assert_eq!(manifest["id"], "typenx-addon-season-centralizer");
        assert_eq!(
            manifest["resources"],
            json!(["catalog", "search", "anime_meta"])
        );
        assert_eq!(manifest["catalogs"].as_array().unwrap().len(), 3);
    }
}
