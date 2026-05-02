import unittest

from typenx_addon_season_centralizer.addon import (
    Source,
    centralize_source_previews,
    decode_refs,
    expand_episode_fallback_refs,
    expand_season_refs,
    fill_missing_episode_air_dates,
)


class FakeClient:
    def __init__(self):
        self.metadata = {
            "anime/52991": {
                "id": "52991",
                "title": "Sousou no Frieren",
                "original_title": None,
                "alternative_titles": ["Frieren: Beyond Journey's End"],
                "episodes": [],
            },
            "anime/21": self.metadata(
                "21",
                "One Piece",
                "1999-10-20",
                [],
                episode_count=0,
            ),
            "anime/12": self.metadata(
                "12",
                "One Piece",
                "1999-10-20",
                [{"id": "103482", "number": 1}],
                episode_count=None,
            ),
        }
        self.search = {
            "Sousou no Frieren": {
                "items": [
                    {
                        "id": "52991",
                        "title": "Sousou no Frieren",
                        "poster": None,
                        "year": 2023,
                        "content_type": "anime",
                    },
                    {
                        "id": "59978",
                        "title": "Sousou no Frieren 2nd Season",
                        "poster": None,
                        "year": 2026,
                        "content_type": "anime",
                    },
                    {
                        "id": "56885",
                        "title": "Sousou no Frieren: Marumaru no Mahou",
                        "poster": None,
                        "year": 2023,
                        "content_type": "anime",
                    },
                ]
            },
            "Frieren: Beyond Journey's End": {"items": []},
            "One Piece": {
                "items": [
                    {
                        "id": "12",
                        "title": "One Piece",
                        "poster": None,
                        "year": 1999,
                        "content_type": "anime",
                    },
                    {
                        "id": "6827",
                        "title": "One Piece Film: Z",
                        "poster": None,
                        "year": 2012,
                        "content_type": "anime",
                    },
                ]
            },
        }

    def get_json(self, _base_url, path):
        return self.metadata[path]

    def post_json(self, _base_url, path, body):
        if path != "search":
            raise AssertionError(f"unexpected path: {path}")
        return self.search.get(body["query"], {"items": []})

    @staticmethod
    def metadata(anime_id, title, start_date, episodes, episode_count):
        return {
            "id": anime_id,
            "title": title,
            "original_title": None,
            "alternative_titles": [],
            "synopsis": None,
            "description": None,
            "poster": None,
            "banner": None,
            "year": int(start_date[:4]),
            "season": None,
            "season_year": int(start_date[:4]),
            "status": "currently_airing",
            "content_type": "anime",
            "source": None,
            "duration_minutes": None,
            "episode_count": episode_count,
            "score": None,
            "rank": None,
            "popularity": None,
            "rating": None,
            "genres": [],
            "tags": [],
            "authors": [],
            "studios": [],
            "staff": [],
            "country_of_origin": "JP",
            "start_date": start_date,
            "end_date": None,
            "site_url": None,
            "trailer_url": None,
            "external_links": [],
            "episodes": episodes,
            "updated_at": None,
        }


class SeasonCentralizerTests(unittest.TestCase):
    def test_groups_split_seasons_inside_each_source(self):
        source = Source(key="mal", base_url="http://127.0.0.1:8787")
        previews = [
            (
                source,
                {
                    "id": "1",
                    "title": "Attack on Titan",
                    "poster": None,
                    "year": 2013,
                    "content_type": "anime",
                },
            ),
            (
                source,
                {
                    "id": "2",
                    "title": "Attack on Titan Season 2",
                    "poster": None,
                    "year": 2017,
                    "content_type": "anime",
                },
            ),
        ]

        items = centralize_source_previews(previews)

        self.assertEqual(len(items), 1)
        self.assertEqual(items[0]["title"], "Attack on Titan")
        self.assertEqual(
            decode_refs(items[0]["id"]),
            [{"source": "mal", "id": "1"}, {"source": "mal", "id": "2"}],
        )

    def test_expands_metadata_refs_with_matching_later_seasons(self):
        source = Source(key="mal", base_url="http://127.0.0.1:8787")

        refs = expand_season_refs(
            FakeClient(),
            [source],
            [{"source": "mal", "id": "52991"}],
        )

        self.assertEqual(
            refs,
            [{"source": "mal", "id": "52991"}, {"source": "mal", "id": "59978"}],
        )

    def test_fills_missing_episode_air_dates_from_season_start(self):
        first = self._metadata("52991", "Sousou no Frieren", "2023-09-29", 2)
        second = self._metadata("59978", "Sousou no Frieren 2nd Season", "2026-01-16", 2)
        combined = {
            "episodes": [
                {
                    "id": "52991:1",
                    "anime_id": "central:52991,59978",
                    "season_number": 1,
                    "number": 1,
                    "title": "Episode 1",
                    "synopsis": None,
                    "thumbnail": None,
                    "aired_at": None,
                },
                {
                    "id": "59978:2",
                    "anime_id": "central:52991,59978",
                    "season_number": 2,
                    "number": 2,
                    "title": "Episode 2",
                    "synopsis": None,
                    "thumbnail": None,
                    "aired_at": None,
                },
            ]
        }

        filled = fill_missing_episode_air_dates(combined, [second, first])

        self.assertEqual(filled["episodes"][0]["aired_at"], "2023-09-29T00:00:00Z")
        self.assertEqual(filled["episodes"][1]["aired_at"], "2026-01-23T00:00:00Z")

    def test_expands_episode_fallback_refs_from_other_sources_when_empty(self):
        sources = [
            Source(key="mal", base_url="http://127.0.0.1:8787"),
            Source(key="kitsu", base_url="http://127.0.0.1:8789"),
        ]
        client = FakeClient()
        refs = [{"source": "mal", "id": "21"}]

        fallback_refs = expand_episode_fallback_refs(
            client,
            sources,
            refs,
            [client.get_json("", "anime/21")],
        )

        self.assertEqual(fallback_refs, [{"source": "kitsu", "id": "12"}])

    def _metadata(self, anime_id, title, start_date, episode_count):
        return {
            "id": anime_id,
            "title": title,
            "original_title": None,
            "alternative_titles": [],
            "synopsis": None,
            "description": None,
            "poster": None,
            "banner": None,
            "year": int(start_date[:4]),
            "season": None,
            "season_year": int(start_date[:4]),
            "status": None,
            "content_type": "anime",
            "source": None,
            "duration_minutes": None,
            "episode_count": episode_count,
            "score": None,
            "rank": None,
            "popularity": None,
            "rating": None,
            "genres": [],
            "tags": [],
            "authors": [],
            "studios": [],
            "staff": [],
            "country_of_origin": "JP",
            "start_date": start_date,
            "end_date": None,
            "site_url": None,
            "trailer_url": None,
            "external_links": [],
            "episodes": [],
            "updated_at": None,
        }


if __name__ == "__main__":
    unittest.main()
