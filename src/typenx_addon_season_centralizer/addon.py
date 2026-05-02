from __future__ import annotations

import base64
import json
import os
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from typing import Any
from urllib.error import URLError
from urllib.parse import quote, urljoin
from urllib.request import Request, urlopen

from typenx_addon_python_sdk import (
    AnimeMetadata,
    AnimePreview,
    CatalogRequest,
    CatalogResponse,
    SearchRequest,
    base_show_title,
    combine_anime_seasons,
    create_typenx_addon,
    normalize_title_key,
    season_number_of,
    serve_typenx_addon,
)

DEFAULT_SOURCES = [
    "http://127.0.0.1:8787",
    "http://127.0.0.1:8788",
    "http://127.0.0.1:8789",
]
TVMAZE_BASE_URL = "https://api.tvmaze.com"


@dataclass(frozen=True)
class Source:
    key: str
    base_url: str


def create_addon():
    sources = configured_sources()
    client = UpstreamClient()

    def catalog(request: CatalogRequest) -> CatalogResponse:
        items = collect_previews(
            sources,
            lambda source: client.post_json(
                source.base_url,
                "catalog",
                {
                    "catalog_id": request["catalog_id"],
                    "skip": request.get("skip"),
                    "limit": request.get("limit"),
                    "query": request.get("query"),
                },
            ),
        )
        return {"items": centralize_source_previews(items)}

    def search(request: SearchRequest) -> CatalogResponse:
        items = collect_previews(
            sources,
            lambda source: client.post_json(
                source.base_url,
                "search",
                {
                    "query": request["query"],
                    "limit": request.get("limit"),
                },
            ),
        )
        return {"items": centralize_source_previews(items)}

    def anime(anime_id: str) -> AnimeMetadata:
        refs = decode_refs(anime_id)
        refs = expand_season_refs(client, sources, refs)
        seasons = [
            client.get_json(source_by_key(sources, ref["source"]).base_url, f"anime/{ref['id']}")
            for ref in refs
        ]
        original_seasons = list(seasons)
        if not has_episode_rows(seasons):
            fallback_refs = expand_episode_fallback_refs(client, sources, refs, seasons)
            fallback_seasons = [
                client.get_json(source_by_key(sources, ref["source"]).base_url, f"anime/{ref['id']}")
                for ref in fallback_refs
            ]
            if has_episode_rows(fallback_seasons):
                refs = fallback_refs
                seasons = fallback_seasons
        thumbnail_sources = [
            *seasons,
            *original_seasons,
            *collect_thumbnail_backup_seasons(client, sources, refs, [*seasons, *original_seasons]),
            *collect_tvmaze_episode_image_sources(client, [*seasons, *original_seasons]),
        ]
        combined = combine_anime_seasons(seasons)
        combined = fill_missing_episode_air_dates(combined, seasons)
        return fill_missing_episode_thumbnails(combined, thumbnail_sources)

    return create_typenx_addon(
        manifest={
            "id": "typenx-addon-season-centralizer",
            "name": "Season Centralizer",
            "version": "0.1.0",
            "description": "Combines split anime seasons from MAL, AniList, and Kitsu into one show.",
            "icon": None,
            "resources": ["catalog", "search", "anime_meta"],
            "catalogs": [
                {"id": "popular", "name": "Popular Anime", "content_type": "anime", "filters": []},
                {"id": "airing", "name": "Airing Anime", "content_type": "anime", "filters": []},
                {"id": "trending", "name": "Trending Anime", "content_type": "anime", "filters": []},
            ],
        },
        handlers={"catalog": catalog, "search": search, "anime": anime},
    )


class UpstreamClient:
    def get_json(self, base_url: str, path: str) -> Any:
        return self._request(base_url, path)

    def post_json(self, base_url: str, path: str, body: dict[str, Any]) -> Any:
        return self._request(base_url, path, body)

    def _request(self, base_url: str, path: str, body: dict[str, Any] | None = None) -> Any:
        data = None if body is None else json.dumps(body).encode("utf-8")
        request = Request(
            urljoin(base_url.rstrip("/") + "/", path),
            data=data,
            headers={"content-type": "application/json", "accept": "application/json"},
            method="POST" if body is not None else "GET",
        )
        with urlopen(request, timeout=12) as response:
            return json.loads(response.read().decode("utf-8"))


def collect_previews(sources: list[Source], load) -> list[tuple[Source, AnimePreview]]:
    items: list[tuple[Source, AnimePreview]] = []
    for source in sources:
        try:
            response = load(source)
        except (OSError, URLError, TimeoutError):
            continue
        for item in response.get("items", []):
            items.append((source, item))
    return items


def expand_season_refs(
    client: UpstreamClient,
    sources: list[Source],
    refs: list[dict[str, str]],
) -> list[dict[str, str]]:
    seasons = [
        (
            source_by_key(sources, ref["source"]),
            client.get_json(source_by_key(sources, ref["source"]).base_url, f"anime/{ref['id']}"),
        )
        for ref in refs
    ]
    title_keys_by_source: dict[str, set[str]] = {}
    queries_by_source: dict[str, set[str]] = {}
    for source, metadata in seasons:
        titles = [
            metadata["title"],
            *metadata.get("alternative_titles", []),
            metadata.get("original_title"),
        ]
        normalized_titles = [
            base_show_title(title)
            for title in titles
            if isinstance(title, str) and title.strip()
        ]
        title_keys_by_source.setdefault(source.key, set()).update(
            normalize_title_key(title) for title in normalized_titles
        )
        queries_by_source.setdefault(source.key, set()).update(normalized_titles)

    expanded = list(refs)
    seen = {(ref["source"], ref["id"]) for ref in refs}
    for source in {source for source, _ in seasons}:
        source_keys = title_keys_by_source.get(source.key, set())
        for query in sorted(queries_by_source.get(source.key, set())):
            try:
                response = client.post_json(
                    source.base_url,
                    "search",
                    {"query": query, "limit": 20},
                )
            except (OSError, URLError, TimeoutError):
                continue
            for item in response.get("items", []):
                if normalize_title_key(item["title"]) not in source_keys:
                    continue
                key = (source.key, item["id"])
                if key in seen:
                    continue
                seen.add(key)
                expanded.append({"source": source.key, "id": item["id"]})

    return expanded


def expand_episode_fallback_refs(
    client: UpstreamClient,
    sources: list[Source],
    refs: list[dict[str, str]],
    seasons: list[AnimeMetadata],
) -> list[dict[str, str]]:
    used_sources = {ref["source"] for ref in refs}
    title_keys = metadata_title_keys(seasons)
    queries = metadata_queries(seasons)
    if not title_keys or not queries:
        return refs

    fallback_refs: list[dict[str, str]] = []
    for source in sources:
        if source.key in used_sources:
            continue
        for query in sorted(queries):
            try:
                response = client.post_json(
                    source.base_url,
                    "search",
                    {"query": query, "limit": 20},
                )
            except (OSError, URLError, TimeoutError):
                continue
            for item in response.get("items", []):
                if normalize_title_key(item["title"]) in title_keys:
                    fallback_refs.append({"source": source.key, "id": item["id"]})
                    break
            if fallback_refs and fallback_refs[-1]["source"] == source.key:
                break

    return fallback_refs or refs


def metadata_title_keys(seasons: list[AnimeMetadata]) -> set[str]:
    return {
        normalize_title_key(title)
        for title in metadata_titles(seasons)
    }


def metadata_queries(seasons: list[AnimeMetadata]) -> set[str]:
    return {base_show_title(title) for title in metadata_titles(seasons)}


def metadata_titles(seasons: list[AnimeMetadata]) -> list[str]:
    titles: list[str] = []
    for metadata in seasons:
        titles.extend(
            title
            for title in [
                metadata["title"],
                *metadata.get("alternative_titles", []),
                metadata.get("original_title"),
            ]
            if isinstance(title, str) and title.strip()
        )
    return titles


def has_episode_rows(seasons: list[AnimeMetadata]) -> bool:
    return any(season.get("episodes") for season in seasons)


def collect_thumbnail_backup_seasons(
    client: UpstreamClient,
    sources: list[Source],
    refs: list[dict[str, str]],
    seasons: list[AnimeMetadata],
) -> list[AnimeMetadata]:
    used_sources = {ref["source"] for ref in refs}
    title_keys = metadata_title_keys(seasons)
    queries = metadata_queries(seasons)
    if not title_keys or not queries:
        return []

    backups: list[AnimeMetadata] = []
    seen: set[tuple[str, str]] = {(ref["source"], ref["id"]) for ref in refs}
    for source in sources:
        if source.key in used_sources:
            continue
        for query in sorted(queries):
            try:
                response = client.post_json(
                    source.base_url,
                    "search",
                    {"query": query, "limit": 20},
                )
            except (OSError, URLError, TimeoutError):
                continue
            for item in response.get("items", []):
                if normalize_title_key(item["title"]) not in title_keys:
                    continue
                key = (source.key, item["id"])
                if key in seen:
                    continue
                seen.add(key)
                try:
                    backups.append(client.get_json(source.base_url, f"anime/{item['id']}"))
                except (OSError, URLError, TimeoutError):
                    continue
    return backups


def collect_tvmaze_episode_image_sources(
    client: UpstreamClient,
    seasons: list[AnimeMetadata],
) -> list[AnimeMetadata]:
    if os.environ.get("TYPENX_TVMAZE_EPISODE_IMAGES", "1").lower() in {"0", "false", "no"}:
        return []

    title_keys = metadata_title_keys(seasons)
    queries = metadata_queries(seasons)
    years = {
        year
        for metadata in seasons
        for year in [metadata.get("season_year"), metadata.get("year"), year_of(metadata.get("start_date"))]
        if isinstance(year, int)
    }
    candidates: dict[int, dict[str, Any]] = {}
    for query in sorted(queries):
        try:
            response = client.get_json(TVMAZE_BASE_URL, f"search/shows?q={quote(query)}")
        except (OSError, URLError, TimeoutError):
            continue
        for result in response:
            show = result.get("show") if isinstance(result, dict) else None
            if not isinstance(show, dict):
                continue
            show_id = show.get("id")
            name = show.get("name")
            if not isinstance(show_id, int) or not isinstance(name, str):
                continue
            if normalize_title_key(name) not in title_keys:
                continue
            current = candidates.get(show_id)
            if not current or tvmaze_show_score(show, years) > tvmaze_show_score(current, years):
                candidates[show_id] = show

    if not candidates:
        return []

    best_show = max(candidates.values(), key=lambda show: tvmaze_show_score(show, years))
    try:
        episodes = client.get_json(TVMAZE_BASE_URL, f"shows/{best_show['id']}/episodes")
    except (OSError, URLError, TimeoutError):
        return []

    mapped_episodes = []
    for episode in episodes:
        image = episode.get("image") if isinstance(episode, dict) else None
        thumbnail = image.get("original") or image.get("medium") if isinstance(image, dict) else None
        if not thumbnail:
            continue
        mapped_episodes.append(
            {
                "id": str(episode.get("id")),
                "anime_id": str(best_show["id"]),
                "season_number": None,
                "number": episode.get("number") if isinstance(episode.get("number"), int) else 0,
                "title": episode.get("name") if isinstance(episode.get("name"), str) else None,
                "synopsis": None,
                "thumbnail": thumbnail,
                "aired_at": iso_date_string(episode.get("airdate")),
                "source": "TVMaze",
            }
        )

    if not mapped_episodes:
        return []

    premiered = best_show.get("premiered") if isinstance(best_show.get("premiered"), str) else None
    image = best_show.get("image") if isinstance(best_show.get("image"), dict) else {}
    return [
        {
            "id": str(best_show["id"]),
            "title": best_show.get("name") or next(iter(metadata_titles(seasons))),
            "original_title": None,
            "alternative_titles": [],
            "synopsis": None,
            "description": None,
            "poster": image.get("original") or image.get("medium"),
            "banner": None,
            "year": year_of(premiered),
            "season": None,
            "season_year": year_of(premiered),
            "status": None,
            "content_type": "anime",
            "source": "TVMaze",
            "duration_minutes": None,
            "episode_count": len(mapped_episodes),
            "score": None,
            "rank": None,
            "popularity": None,
            "rating": None,
            "genres": [],
            "tags": [],
            "authors": [],
            "studios": [],
            "staff": [],
            "country_of_origin": None,
            "start_date": premiered,
            "end_date": None,
            "site_url": best_show.get("url") if isinstance(best_show.get("url"), str) else None,
            "trailer_url": None,
            "external_links": [],
            "episodes": mapped_episodes,
            "updated_at": None,
        }
    ]


def tvmaze_show_score(show: dict[str, Any], years: set[int]) -> int:
    score = 0
    if show.get("language") == "Japanese":
        score += 100
    premiered_year = year_of(show.get("premiered") if isinstance(show.get("premiered"), str) else None)
    if premiered_year in years:
        score += 50
    elif premiered_year and years:
        score += max(0, 20 - min(abs(premiered_year - year) for year in years))
    return score


def fill_missing_episode_air_dates(
    combined: AnimeMetadata,
    seasons: list[AnimeMetadata],
) -> AnimeMetadata:
    starts = season_start_dates(seasons)
    if not starts:
        return combined

    for episode in combined.get("episodes", []):
        if episode.get("aired_at"):
            continue
        season_number = episode.get("season_number")
        episode_number = episode.get("number")
        start = starts.get(season_number)
        if not start or not isinstance(episode_number, int) or episode_number < 1:
            continue
        episode["aired_at"] = iso_date_at_midnight(start + timedelta(days=(episode_number - 1) * 7))

    return combined


def fill_missing_episode_thumbnails(
    combined: AnimeMetadata,
    sources: list[AnimeMetadata],
) -> AnimeMetadata:
    episode_thumbnails = episode_thumbnail_lookup(sources)
    season_art = season_artwork_lookup(sources)
    show_art = next(
        (
            artwork
            for metadata in sources
            for artwork in [metadata.get("poster"), metadata.get("banner")]
            if artwork
        ),
        None,
    )

    for episode in combined.get("episodes", []):
        if episode.get("thumbnail"):
            continue
        season_number = episode.get("season_number")
        episode_number = episode.get("number")
        title_key = normalize_title_key(episode["title"]) if episode.get("title") else None
        thumbnail = episode_thumbnails.get((season_number, episode_number))
        if not thumbnail and title_key:
            thumbnail = episode_thumbnails.get((season_number, title_key))
        if not thumbnail:
            thumbnail = episode_thumbnails.get((None, episode_number))
        if not thumbnail and title_key:
            thumbnail = episode_thumbnails.get((None, title_key))
        if not thumbnail:
            aired_at = date_key(episode.get("aired_at"))
            if aired_at:
                thumbnail = episode_thumbnails.get((None, aired_at))
        if not thumbnail:
            thumbnail = season_art.get(season_number) or show_art
        if thumbnail:
            episode["thumbnail"] = thumbnail

    return combined


def episode_thumbnail_lookup(sources: list[AnimeMetadata]) -> dict[tuple[int | None, int | str], str]:
    thumbnails: dict[tuple[int | None, int | str], str] = {}
    for season_number, metadata in inferred_season_metadata(sources):
        for episode in metadata.get("episodes", []):
            thumbnail = episode.get("thumbnail")
            if not thumbnail:
                continue
            episode_number = episode.get("number")
            effective_season_number = episode.get("season_number") or season_number
            if isinstance(episode_number, int):
                thumbnails.setdefault((effective_season_number, episode_number), thumbnail)
                thumbnails.setdefault((None, episode_number), thumbnail)
            title = episode.get("title")
            if isinstance(title, str) and title.strip():
                title_key = normalize_title_key(title)
                thumbnails.setdefault((effective_season_number, title_key), thumbnail)
                thumbnails.setdefault((None, title_key), thumbnail)
            aired_at = date_key(episode.get("aired_at"))
            if aired_at:
                thumbnails.setdefault((None, aired_at), thumbnail)
    return thumbnails


def season_artwork_lookup(sources: list[AnimeMetadata]) -> dict[int, str]:
    artwork: dict[int, str] = {}
    for season_number, metadata in inferred_season_metadata(sources):
        image = metadata.get("poster") or metadata.get("banner")
        if image:
            artwork.setdefault(season_number, image)
    return artwork


def inferred_season_metadata(seasons: list[AnimeMetadata]) -> list[tuple[int, AnimeMetadata]]:
    inferred: list[tuple[int, AnimeMetadata]] = []
    used_season_numbers: set[int] = set()
    for season in sorted_seasons(seasons):
        season_number = season_number_of(season["title"]) or next_later_season_number(used_season_numbers)
        used_season_numbers.add(season_number)
        inferred.append((season_number, season))
    return inferred


def season_start_dates(seasons: list[AnimeMetadata]) -> dict[int, date]:
    starts: dict[int, date] = {}
    used_season_numbers: set[int] = set()
    for season in sorted_seasons(seasons):
        season_number = season_number_of(season["title"]) or next_later_season_number(used_season_numbers)
        used_season_numbers.add(season_number)
        start = parse_date(season.get("start_date"))
        if start:
            starts[season_number] = start
    return starts


def sorted_seasons(seasons: list[AnimeMetadata]) -> list[AnimeMetadata]:
    return sorted(
        seasons,
        key=lambda item: (
            item.get("season_year") or item.get("year") or 0,
            item.get("start_date") or "",
            season_number_of(item["title"]) or 1,
        ),
    )


def next_later_season_number(used: set[int]) -> int:
    return max(used, default=0) + 1


def parse_date(value: str | None) -> date | None:
    if not value:
        return None
    try:
        return date.fromisoformat(value[:10])
    except ValueError:
        return None


def year_of(value: str | None) -> int | None:
    parsed = parse_date(value)
    return parsed.year if parsed else None


def iso_date_string(value: str | None) -> str | None:
    parsed = parse_date(value)
    return iso_date_at_midnight(parsed) if parsed else None


def date_key(value: str | None) -> str | None:
    parsed = parse_date(value)
    return parsed.isoformat() if parsed else None


def iso_date_at_midnight(value: date) -> str:
    return datetime.combine(value, datetime.min.time(), timezone.utc).isoformat().replace("+00:00", "Z")


def centralize_source_previews(items: list[tuple[Source, AnimePreview]]) -> list[AnimePreview]:
    groups: dict[tuple[str, str], list[tuple[Source, AnimePreview]]] = {}
    for source, item in items:
        key = (source.key, normalize_title_key(item["title"]))
        groups.setdefault(key, []).append((source, item))

    centralized: list[AnimePreview] = []
    for group in groups.values():
        sorted_group = sorted(
            group,
            key=lambda entry: (
                season_number_of(entry[1]["title"]) or 1,
                entry[1].get("year") or 0,
                entry[1]["title"],
            ),
        )
        primary_source, primary = sorted_group[0]
        refs = [{"source": source.key, "id": item["id"]} for source, item in sorted_group]
        centralized.append(
            {
                **primary,
                "id": encode_refs(refs),
                "title": base_show_title(primary["title"]),
                "season_entries": [
                    {
                        "id": item["id"],
                        "title": item["title"],
                        "season_number": season_number_of(item["title"]),
                        "year": item.get("year"),
                        "episode_count": None,
                        "source": source.key,
                    }
                    for source, item in sorted_group
                ],
                "external_source": primary_source.key,
            }
        )
    return centralized


def configured_sources() -> list[Source]:
    raw_sources = os.environ.get("TYPENX_SEASON_SOURCES", ",".join(DEFAULT_SOURCES))
    urls = [value.strip().rstrip("/") for value in raw_sources.split(",") if value.strip()]
    return [Source(key=f"source-{index + 1}", base_url=url) for index, url in enumerate(urls)]


def source_by_key(sources: list[Source], key: str) -> Source:
    for source in sources:
        if source.key == key:
            return source
    raise ValueError(f"Unknown upstream source: {key}")


def encode_refs(refs: list[dict[str, str]]) -> str:
    payload = json.dumps(refs, separators=(",", ":")).encode("utf-8")
    token = base64.urlsafe_b64encode(payload).decode("ascii").rstrip("=")
    return f"central:{token}"


def decode_refs(anime_id: str) -> list[dict[str, str]]:
    if not anime_id.startswith("central:"):
        raise ValueError("Season Centralizer anime ids must start with central:")
    token = anime_id[len("central:") :]
    padding = "=" * (-len(token) % 4)
    refs = json.loads(base64.urlsafe_b64decode(token + padding).decode("utf-8"))
    if not isinstance(refs, list) or not refs:
        raise ValueError("Season Centralizer anime id did not include source refs")
    return refs


def main() -> None:
    serve_typenx_addon(create_addon(), port=int(os.environ.get("PORT", "8790")))
