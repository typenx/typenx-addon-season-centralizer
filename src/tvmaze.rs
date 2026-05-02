use crate::centralization::{
    date_key, iso_date_at_midnight, metadata_queries, metadata_title_keys, string_field, year_of,
};
use crate::models::AnimeMetadata;
use crate::source_client::UpstreamClient;
use serde_json::{json, Value};
use std::collections::HashMap;

const TVMAZE_BASE_URL: &str = "https://api.tvmaze.com";

pub async fn collect_tvmaze_episode_image_sources(
    client: &UpstreamClient,
    seasons: &[AnimeMetadata],
) -> Vec<AnimeMetadata> {
    if std::env::var("TYPENX_TVMAZE_EPISODE_IMAGES")
        .unwrap_or_else(|_| "1".to_string())
        .to_lowercase()
        .as_str()
        .is_one_of(&["0", "false", "no"])
    {
        return vec![];
    }

    let title_keys = metadata_title_keys(seasons);
    let queries = metadata_queries(seasons);
    let years = seasons
        .iter()
        .flat_map(|metadata| {
            [
                metadata.get("season_year").and_then(Value::as_i64),
                metadata.get("year").and_then(Value::as_i64),
                year_of(string_field(metadata, "start_date")),
            ]
        })
        .flatten()
        .collect::<Vec<_>>();

    let mut candidates: HashMap<i64, Value> = HashMap::new();
    let mut sorted_queries = queries.into_iter().collect::<Vec<_>>();
    sorted_queries.sort();
    for query in sorted_queries {
        let path = format!("search/shows?q={}", urlencoding::encode(&query));
        let Ok(response) = client.get_json(TVMAZE_BASE_URL, &path).await else {
            continue;
        };
        let Some(results) = response.as_array() else {
            continue;
        };
        for result in results {
            let Some(show) = result.get("show").filter(|show| show.is_object()) else {
                continue;
            };
            let Some(show_id) = show.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let Some(name) = show.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !title_keys.contains(&crate::centralization::normalize_title_key(name)) {
                continue;
            }
            let replace = candidates.get(&show_id).is_none_or(|current| {
                tvmaze_show_score(show, &years) > tvmaze_show_score(current, &years)
            });
            if replace {
                candidates.insert(show_id, show.clone());
            }
        }
    }

    let Some(best_show) = candidates
        .values()
        .max_by_key(|show| tvmaze_show_score(show, &years))
        .cloned()
    else {
        return vec![];
    };
    let Some(best_id) = best_show.get("id").and_then(Value::as_i64) else {
        return vec![];
    };
    let Ok(episodes) = client
        .get_json(TVMAZE_BASE_URL, &format!("shows/{best_id}/episodes"))
        .await
    else {
        return vec![];
    };

    let mapped_episodes = episodes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let image = episode.get("image").and_then(Value::as_object)?;
            let thumbnail = image.get("original").or_else(|| image.get("medium"))?.as_str()?;
            Some(json!({
                "id": episode.get("id").map(|id| id.to_string().trim_matches('"').to_string()).unwrap_or_default(),
                "anime_id": best_id.to_string(),
                "season_number": null,
                "number": episode.get("number").and_then(Value::as_i64).unwrap_or(0),
                "title": episode.get("name").and_then(Value::as_str),
                "synopsis": null,
                "thumbnail": thumbnail,
                "aired_at": date_key(episode.get("airdate").and_then(Value::as_str)).and_then(|value| {
                    chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok().map(iso_date_at_midnight)
                }),
                "source": "TVMaze"
            }))
        })
        .collect::<Vec<_>>();

    if mapped_episodes.is_empty() {
        return vec![];
    }

    let premiered = best_show.get("premiered").and_then(Value::as_str);
    let image = best_show.get("image").and_then(Value::as_object);
    vec![json!({
        "id": best_id.to_string(),
        "title": best_show.get("name").and_then(Value::as_str).unwrap_or_else(|| {
            seasons.iter().find_map(|metadata| string_field(metadata, "title")).unwrap_or("")
        }),
        "original_title": null,
        "alternative_titles": [],
        "synopsis": null,
        "description": null,
        "poster": image.and_then(|image| image.get("original").or_else(|| image.get("medium"))).cloned().unwrap_or(Value::Null),
        "banner": null,
        "year": year_of(premiered),
        "season": null,
        "season_year": year_of(premiered),
        "status": null,
        "content_type": "anime",
        "source": "TVMaze",
        "duration_minutes": null,
        "episode_count": mapped_episodes.len(),
        "score": null,
        "rank": null,
        "popularity": null,
        "rating": null,
        "genres": [],
        "tags": [],
        "authors": [],
        "studios": [],
        "staff": [],
        "country_of_origin": null,
        "start_date": premiered,
        "end_date": null,
        "site_url": best_show.get("url").and_then(Value::as_str),
        "trailer_url": null,
        "external_links": [],
        "episodes": mapped_episodes,
        "updated_at": null
    })]
}

fn tvmaze_show_score(show: &Value, years: &[i64]) -> i64 {
    let mut score = 0;
    if show.get("language").and_then(Value::as_str) == Some("Japanese") {
        score += 100;
    }
    let premiered_year = year_of(show.get("premiered").and_then(Value::as_str));
    if let Some(premiered_year) = premiered_year {
        if years.contains(&premiered_year) {
            score += 50;
        } else if !years.is_empty() {
            let distance = years
                .iter()
                .map(|year| (premiered_year - year).abs())
                .min()
                .unwrap_or(20);
            score += 0.max(20 - distance);
        }
    }
    score
}

trait OneOf {
    fn is_one_of(self, values: &[&str]) -> bool;
}

impl OneOf for &str {
    fn is_one_of(self, values: &[&str]) -> bool {
        values.contains(&self)
    }
}
