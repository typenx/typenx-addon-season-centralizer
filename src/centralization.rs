use crate::models::{AnimeMetadata, AnimePreview, Source, SourceRef};
use anyhow::{anyhow, Result};
use base64::prelude::*;
use chrono::{DateTime, Days, NaiveDate, Utc};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub fn base_show_title(title: &str) -> String {
    let mut value = title.to_string();
    for pattern in season_patterns() {
        value = pattern.replace_all(&value, "").to_string();
    }
    value = ordinal_word_pattern().replace_all(&value, "").to_string();
    value = trailing_season_pattern()
        .replace_all(&value, "")
        .to_string();
    whitespace_pattern()
        .replace_all(&value, " ")
        .trim()
        .to_string()
}

pub fn normalize_title_key(title: &str) -> String {
    non_alnum_pattern()
        .replace_all(&base_show_title(title).to_lowercase(), "")
        .to_string()
}

pub fn season_number_of(title: &str) -> Option<i64> {
    for pattern in season_number_patterns() {
        if let Some(caps) = pattern.captures(title) {
            return caps.get(1)?.as_str().parse().ok();
        }
    }
    ordinal_word_pattern().captures(title).and_then(|caps| {
        match &caps.get(1)?.as_str().to_lowercase()[..] {
            "second" => Some(2),
            "third" => Some(3),
            "fourth" => Some(4),
            "fifth" => Some(5),
            "sixth" => Some(6),
            "seventh" => Some(7),
            "eighth" => Some(8),
            "ninth" => Some(9),
            "tenth" => Some(10),
            _ => None,
        }
    })
}

pub fn centralize_source_previews(items: &[(Source, AnimePreview)]) -> Vec<AnimePreview> {
    let mut groups: HashMap<(String, String), Vec<(Source, AnimePreview)>> = HashMap::new();
    for (source, item) in items {
        let Some(title) = string_field(item, "title") else {
            continue;
        };
        groups
            .entry((source.key.clone(), normalize_title_key(title)))
            .or_default()
            .push((source.clone(), item.clone()));
    }

    groups
        .into_values()
        .filter_map(|mut group| {
            group.sort_by(|a, b| preview_sort_key(&a.1).cmp(&preview_sort_key(&b.1)));
            let (primary_source, primary) = group.first()?.clone();
            let refs: Vec<SourceRef> = group
                .iter()
                .filter_map(|(source, item)| {
                    Some(SourceRef {
                        source: source.key.clone(),
                        id: string_field(item, "id")?.to_string(),
                    })
                })
                .collect();
            let mut output = primary.as_object()?.clone();
            output.insert("id".to_string(), Value::String(encode_refs(&refs)));
            output.insert(
                "title".to_string(),
                Value::String(base_show_title(string_field(&primary, "title")?)),
            );
            output.insert(
                "season_entries".to_string(),
                Value::Array(
                    group
                        .iter()
                        .filter_map(|(source, item)| {
                            let title = string_field(item, "title")?;
                            Some(json!({
                                "id": string_field(item, "id")?,
                                "title": title,
                                "season_number": season_number_of(title),
                                "year": int_field(item, "year"),
                                "episode_count": null,
                                "source": source.key,
                            }))
                        })
                        .collect(),
                ),
            );
            output.insert(
                "external_source".to_string(),
                Value::String(primary_source.key),
            );
            Some(Value::Object(output))
        })
        .collect()
}

pub fn combine_anime_seasons(seasons: &[AnimeMetadata]) -> Result<AnimeMetadata> {
    if seasons.is_empty() {
        return Err(anyhow!("at least one season is required"));
    }
    let mut ordered = seasons.to_vec();
    ordered.sort_by(|a, b| metadata_sort_key(a).cmp(&metadata_sort_key(b)));
    let mut combined = ordered[0]
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("anime metadata must be a JSON object"))?;
    let ids = ordered
        .iter()
        .filter_map(|item| string_field(item, "id"))
        .collect::<Vec<_>>()
        .join(",");
    let central_id = format!("central:{ids}");
    let title = string_field(&ordered[0], "title").unwrap_or_default();
    let base_title = base_show_title(title);
    combined.insert("id".to_string(), Value::String(central_id.clone()));
    combined.insert("title".to_string(), Value::String(base_title.clone()));
    combined.insert(
        "alternative_titles".to_string(),
        strings_array(unique_strings(ordered.iter().flat_map(|item| {
            let mut values = vec![];
            if let Some(title) = string_field(item, "title") {
                if title != base_title {
                    values.push(title.to_string());
                }
            }
            values.extend(string_array(item, "alternative_titles"));
            values
        }))),
    );
    for key in ["genres", "tags", "studios"] {
        combined.insert(
            key.to_string(),
            strings_array(unique_strings(
                ordered.iter().flat_map(|item| string_array(item, key)),
            )),
        );
    }
    combined.insert(
        "episodes".to_string(),
        Value::Array(combined_episodes(&ordered, &central_id)),
    );
    let episode_count = combined
        .get("episodes")
        .and_then(Value::as_array)
        .map(Vec::len)
        .filter(|count| *count > 0)
        .map(|count| count as i64)
        .or_else(|| {
            let total: i64 = ordered
                .iter()
                .filter_map(|item| int_field(item, "episode_count"))
                .sum();
            (total > 0).then_some(total)
        });
    combined.insert("episode_count".to_string(), option_int(episode_count));
    combined.insert(
        "start_date".to_string(),
        first_string_or_null(&ordered, "start_date"),
    );
    combined.insert(
        "end_date".to_string(),
        last_string_or_null(&ordered, "end_date"),
    );
    combined.insert("season".to_string(), Value::Null);
    combined.insert(
        "season_year".to_string(),
        combined.get("year").cloned().unwrap_or(Value::Null),
    );
    Ok(Value::Object(combined))
}

pub fn fill_missing_episode_air_dates(
    mut combined: AnimeMetadata,
    seasons: &[AnimeMetadata],
) -> AnimeMetadata {
    let starts = season_start_dates(seasons);
    if let Some(episodes) = combined.get_mut("episodes").and_then(Value::as_array_mut) {
        for episode in episodes {
            if episode
                .get("aired_at")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_some()
            {
                continue;
            }
            let Some(season_number) = int_field(episode, "season_number") else {
                continue;
            };
            let Some(number) = int_field(episode, "number") else {
                continue;
            };
            if number < 1 {
                continue;
            }
            if let Some(start) = starts.get(&season_number) {
                if let Some(date) = start.checked_add_days(Days::new((number - 1) as u64)) {
                    set_field(
                        episode,
                        "aired_at",
                        Value::String(iso_date_at_midnight(date)),
                    );
                }
            }
        }
    }
    combined
}

pub fn fill_missing_episode_thumbnails(
    mut combined: AnimeMetadata,
    sources: &[AnimeMetadata],
) -> AnimeMetadata {
    let thumbnails = episode_thumbnail_lookup(sources);
    let season_art = season_artwork_lookup(sources);
    let show_art = sources
        .iter()
        .find_map(|metadata| {
            string_field(metadata, "poster").or_else(|| string_field(metadata, "banner"))
        })
        .map(str::to_string);
    if let Some(episodes) = combined.get_mut("episodes").and_then(Value::as_array_mut) {
        for episode in episodes {
            if episode
                .get("thumbnail")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_some()
            {
                continue;
            }
            let season_number = int_field(episode, "season_number");
            let number = int_field(episode, "number");
            let title_key = string_field(episode, "title").map(normalize_title_key);
            let aired_at = date_key(string_field(episode, "aired_at"));
            let thumb = number
                .and_then(|n| {
                    thumbnails
                        .get(&(season_number, LookupKey::Number(n)))
                        .cloned()
                })
                .or_else(|| {
                    title_key.as_ref().and_then(|t| {
                        thumbnails
                            .get(&(season_number, LookupKey::Text(t.clone())))
                            .cloned()
                    })
                })
                .or_else(|| {
                    number.and_then(|n| thumbnails.get(&(None, LookupKey::Number(n))).cloned())
                })
                .or_else(|| {
                    title_key
                        .as_ref()
                        .and_then(|t| thumbnails.get(&(None, LookupKey::Text(t.clone()))).cloned())
                })
                .or_else(|| {
                    aired_at.and_then(|d| thumbnails.get(&(None, LookupKey::Text(d))).cloned())
                })
                .or_else(|| season_number.and_then(|s| season_art.get(&s).cloned()))
                .or_else(|| show_art.clone());
            if let Some(thumb) = thumb {
                set_field(episode, "thumbnail", Value::String(thumb));
            }
        }
    }
    combined
}

pub fn metadata_titles(seasons: &[AnimeMetadata]) -> Vec<String> {
    let mut titles = vec![];
    for metadata in seasons {
        if let Some(title) = string_field(metadata, "title") {
            titles.push(title.to_string());
        }
        titles.extend(string_array(metadata, "alternative_titles"));
        if let Some(title) = string_field(metadata, "original_title") {
            titles.push(title.to_string());
        }
    }
    titles
        .into_iter()
        .filter(|title| !title.trim().is_empty())
        .collect()
}

pub fn metadata_title_keys(seasons: &[AnimeMetadata]) -> HashSet<String> {
    metadata_titles(seasons)
        .iter()
        .map(|title| normalize_title_key(title))
        .collect()
}

pub fn metadata_queries(seasons: &[AnimeMetadata]) -> HashSet<String> {
    metadata_titles(seasons)
        .iter()
        .map(|title| base_show_title(title))
        .collect()
}

pub fn has_episode_rows(seasons: &[AnimeMetadata]) -> bool {
    seasons.iter().any(|season| {
        season
            .get("episodes")
            .and_then(Value::as_array)
            .is_some_and(|eps| !eps.is_empty())
    })
}

pub fn sorted_seasons(seasons: &[AnimeMetadata]) -> Vec<AnimeMetadata> {
    let mut ordered = seasons.to_vec();
    ordered.sort_by(|a, b| metadata_sort_key(a).cmp(&metadata_sort_key(b)));
    ordered
}

pub fn encode_refs(refs: &[SourceRef]) -> String {
    let payload = serde_json::to_vec(refs).expect("source refs serialize");
    format!("central:{}", BASE64_URL_SAFE_NO_PAD.encode(payload))
}

pub fn decode_refs(anime_id: &str) -> Result<Vec<SourceRef>> {
    let token = anime_id
        .strip_prefix("central:")
        .ok_or_else(|| anyhow!("Season Centralizer anime ids must start with central:"))?;
    let refs: Vec<SourceRef> = serde_json::from_slice(&BASE64_URL_SAFE_NO_PAD.decode(token)?)?;
    if refs.is_empty() {
        return Err(anyhow!(
            "Season Centralizer anime id did not include source refs"
        ));
    }
    Ok(refs)
}

pub fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

pub fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

pub fn parse_date(value: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value?.get(..10)?, "%Y-%m-%d").ok()
}

pub fn year_of(value: Option<&str>) -> Option<i64> {
    parse_date(value)
        .map(|date| date.format("%Y").to_string().parse().ok())
        .flatten()
}

pub fn iso_date_at_midnight(value: NaiveDate) -> String {
    let dt = DateTime::<Utc>::from_naive_utc_and_offset(
        value.and_hms_opt(0, 0, 0).expect("midnight"),
        Utc,
    );
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn date_key(value: Option<&str>) -> Option<String> {
    parse_date(value).map(|date| date.format("%Y-%m-%d").to_string())
}

fn preview_sort_key(item: &Value) -> (i64, i64, String) {
    let title = string_field(item, "title").unwrap_or_default();
    (
        season_number_of(title).unwrap_or(1),
        int_field(item, "year").unwrap_or(0),
        title.to_string(),
    )
}

fn metadata_sort_key(item: &Value) -> (i64, String, i64) {
    let title = string_field(item, "title").unwrap_or_default();
    (
        int_field(item, "season_year")
            .or_else(|| int_field(item, "year"))
            .unwrap_or(0),
        string_field(item, "start_date")
            .unwrap_or_default()
            .to_string(),
        season_number_of(title).unwrap_or(1),
    )
}

fn combined_episodes(seasons: &[AnimeMetadata], central_id: &str) -> Vec<Value> {
    let mut episodes = vec![];
    let mut used = HashSet::new();
    for season in seasons {
        let title = string_field(season, "title").unwrap_or_default();
        let season_number =
            season_number_of(title).unwrap_or_else(|| next_later_season_number(&used));
        used.insert(season_number);
        let Some(rows) = season.get("episodes").and_then(Value::as_array) else {
            continue;
        };
        for episode in rows {
            if let Some(mut merged) = episode.as_object().cloned() {
                let episode_id = string_field(episode, "id").unwrap_or_default();
                let season_id = string_field(season, "id").unwrap_or_default();
                merged.insert(
                    "anime_id".to_string(),
                    Value::String(central_id.to_string()),
                );
                if int_field(episode, "season_number").is_none() {
                    merged.insert(
                        "season_number".to_string(),
                        Value::Number(season_number.into()),
                    );
                }
                merged.insert(
                    "id".to_string(),
                    Value::String(format!("{season_id}:{episode_id}")),
                );
                episodes.push(Value::Object(merged));
            }
        }
    }
    episodes
}

fn season_start_dates(seasons: &[AnimeMetadata]) -> HashMap<i64, NaiveDate> {
    let mut starts = HashMap::new();
    let mut used = HashSet::new();
    for season in sorted_seasons(seasons) {
        let title = string_field(&season, "title").unwrap_or_default();
        let season_number =
            season_number_of(title).unwrap_or_else(|| next_later_season_number(&used));
        used.insert(season_number);
        if let Some(start) = parse_date(string_field(&season, "start_date")) {
            starts.insert(season_number, start);
        }
    }
    starts
}

fn inferred_season_metadata(seasons: &[AnimeMetadata]) -> Vec<(i64, AnimeMetadata)> {
    let mut inferred = vec![];
    let mut used = HashSet::new();
    for season in sorted_seasons(seasons) {
        let title = string_field(&season, "title").unwrap_or_default();
        let season_number =
            season_number_of(title).unwrap_or_else(|| next_later_season_number(&used));
        used.insert(season_number);
        inferred.push((season_number, season));
    }
    inferred
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LookupKey {
    Number(i64),
    Text(String),
}

fn episode_thumbnail_lookup(
    sources: &[AnimeMetadata],
) -> HashMap<(Option<i64>, LookupKey), String> {
    let mut thumbnails = HashMap::new();
    for (season_number, metadata) in inferred_season_metadata(sources) {
        let Some(episodes) = metadata.get("episodes").and_then(Value::as_array) else {
            continue;
        };
        for episode in episodes {
            let Some(thumbnail) = string_field(episode, "thumbnail") else {
                continue;
            };
            let effective_season = int_field(episode, "season_number").or(Some(season_number));
            if let Some(number) = int_field(episode, "number") {
                thumbnails
                    .entry((effective_season, LookupKey::Number(number)))
                    .or_insert_with(|| thumbnail.to_string());
                thumbnails
                    .entry((None, LookupKey::Number(number)))
                    .or_insert_with(|| thumbnail.to_string());
            }
            if let Some(title) = string_field(episode, "title") {
                let key = normalize_title_key(title);
                thumbnails
                    .entry((effective_season, LookupKey::Text(key.clone())))
                    .or_insert_with(|| thumbnail.to_string());
                thumbnails
                    .entry((None, LookupKey::Text(key)))
                    .or_insert_with(|| thumbnail.to_string());
            }
            if let Some(aired_at) = date_key(string_field(episode, "aired_at")) {
                thumbnails
                    .entry((None, LookupKey::Text(aired_at)))
                    .or_insert_with(|| thumbnail.to_string());
            }
        }
    }
    thumbnails
}

fn season_artwork_lookup(sources: &[AnimeMetadata]) -> HashMap<i64, String> {
    let mut artwork = HashMap::new();
    for (season_number, metadata) in inferred_season_metadata(sources) {
        if let Some(image) =
            string_field(&metadata, "poster").or_else(|| string_field(&metadata, "banner"))
        {
            artwork
                .entry(season_number)
                .or_insert_with(|| image.to_string());
        }
    }
    artwork
}

fn next_later_season_number(used: &HashSet<i64>) -> i64 {
    used.iter().max().copied().unwrap_or(0) + 1
}

fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = vec![];
    for value in values {
        let normalized = value.trim();
        if !normalized.is_empty() && seen.insert(normalized.to_string()) {
            output.push(normalized.to_string());
        }
    }
    output
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn strings_array(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

fn option_int(value: Option<i64>) -> Value {
    value
        .map(|value| Value::Number(value.into()))
        .unwrap_or(Value::Null)
}

fn first_string_or_null(items: &[Value], key: &str) -> Value {
    items
        .iter()
        .find_map(|item| string_field(item, key))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn last_string_or_null(items: &[Value], key: &str) -> Value {
    items
        .iter()
        .rev()
        .find_map(|item| string_field(item, key))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn set_field(value: &mut Value, key: &str, field: Value) {
    if let Some(map) = value.as_object_mut() {
        map.insert(key.to_string(), field);
    }
}

fn season_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)\b(?:the\s+)?final\s+season\b",
            r"(?i)\bseason\s+\d+\b",
            r"(?i)\bs\d+\b",
            r"(?i)\b\d+(?:st|nd|rd|th)\s+season\b",
            r"(?i)\bpart\s+\d+\b",
            r"(?i)\bcour\s+\d+\b",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).unwrap())
        .collect()
    })
}

fn season_number_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)\bseason\s+(\d+)\b",
            r"(?i)\bs(\d+)\b",
            r"(?i)\b(\d+)(?:st|nd|rd|th)\s+season\b",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).unwrap())
        .collect()
    })
}

fn ordinal_word_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth)\s+season\b")
            .unwrap()
    })
}

fn trailing_season_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\s*[:-]\s*$").unwrap())
}

fn whitespace_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\s+").unwrap())
}

fn non_alnum_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_split_season_titles() {
        assert_eq!(
            base_show_title("Attack on Titan Season 2"),
            "Attack on Titan"
        );
        assert_eq!(
            base_show_title("Sousou no Frieren 2nd Season"),
            "Sousou no Frieren"
        );
        assert_eq!(
            base_show_title("Bleach: Thousand-Year Blood War Part 2"),
            "Bleach: Thousand-Year Blood War"
        );
        assert_eq!(
            normalize_title_key("Attack on Titan: The Final Season"),
            "attackontitan"
        );
        assert_eq!(season_number_of("Haikyu!! Third Season"), Some(3));
    }

    #[test]
    fn groups_split_seasons_inside_each_source() {
        let source = Source {
            key: "mal".into(),
            base_url: "http://127.0.0.1:8787".into(),
        };
        let items = vec![
            (
                source.clone(),
                json!({"id":"1","title":"Attack on Titan","poster":null,"year":2013,"content_type":"anime"}),
            ),
            (
                source.clone(),
                json!({"id":"2","title":"Attack on Titan Season 2","poster":null,"year":2017,"content_type":"anime"}),
            ),
        ];
        let centralized = centralize_source_previews(&items);
        assert_eq!(centralized.len(), 1);
        assert_eq!(centralized[0]["title"], "Attack on Titan");
        assert_eq!(
            decode_refs(centralized[0]["id"].as_str().unwrap()).unwrap(),
            vec![
                SourceRef {
                    source: "mal".into(),
                    id: "1".into()
                },
                SourceRef {
                    source: "mal".into(),
                    id: "2".into()
                },
            ]
        );
    }

    #[test]
    fn merges_metadata_and_numbers_episodes_by_inferred_season() {
        let first = metadata(
            "1",
            "Attack on Titan",
            2013,
            "2013-04-07",
            vec![json!({"id":"e1","number":1,"title":"To You","thumbnail":null,"aired_at":null})],
        );
        let second = metadata(
            "2",
            "Attack on Titan Season 2",
            2017,
            "2017-04-01",
            vec![
                json!({"id":"e2","number":1,"title":"Beast Titan","thumbnail":null,"aired_at":null}),
            ],
        );
        let combined = combine_anime_seasons(&[second, first]).unwrap();
        assert_eq!(combined["id"], "central:1,2");
        assert_eq!(combined["title"], "Attack on Titan");
        assert_eq!(combined["episode_count"], 2);
        assert_eq!(combined["episodes"][0]["id"], "1:e1");
        assert_eq!(combined["episodes"][1]["season_number"], 2);
    }

    #[test]
    fn fills_missing_episode_thumbnails_from_backup_episode() {
        let mut combined = metadata(
            "central",
            "One Piece",
            1999,
            "1999-10-20",
            vec![json!({
                "id":"central:427","anime_id":"central","season_number":13,"number":427,
                "title":"A Special Presentation Related to the Movie!","synopsis":null,"thumbnail":null,"aired_at":null
            })],
        );
        let backup = metadata(
            "backup",
            "One Piece",
            1999,
            "1999-10-20",
            vec![json!({
                "id":"backup:427","anime_id":"backup","season_number":13,"number":427,
                "title":"A Special Presentation Related to the Movie!","synopsis":null,
                "thumbnail":"https://example.test/episode-427.jpg","aired_at":null
            })],
        );
        combined = fill_missing_episode_thumbnails(combined, &[backup]);
        assert_eq!(
            combined["episodes"][0]["thumbnail"],
            "https://example.test/episode-427.jpg"
        );
    }

    fn metadata(id: &str, title: &str, year: i64, start_date: &str, episodes: Vec<Value>) -> Value {
        json!({
            "id": id, "title": title, "original_title": null, "alternative_titles": [],
            "synopsis": null, "description": null, "poster": null, "banner": null,
            "year": year, "season": null, "season_year": year, "status": null,
            "content_type": "anime", "source": null, "duration_minutes": null,
            "episode_count": null, "score": null, "rank": null, "popularity": null,
            "rating": null, "genres": [], "tags": [], "authors": [], "studios": [],
            "staff": [], "country_of_origin": "JP", "start_date": start_date,
            "end_date": null, "site_url": null, "trailer_url": null,
            "external_links": [], "episodes": episodes, "updated_at": null
        })
    }
}
