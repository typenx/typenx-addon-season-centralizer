import unittest

from typenx_addon_season_centralizer.addon import (
    Source,
    centralize_source_previews,
    decode_refs,
    expand_season_refs,
)


class FakeClient:
    def __init__(self):
        self.metadata = {
            "anime/52991": {
                "id": "52991",
                "title": "Sousou no Frieren",
                "original_title": None,
                "alternative_titles": ["Frieren: Beyond Journey's End"],
            }
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
        }

    def get_json(self, _base_url, path):
        return self.metadata[path]

    def post_json(self, _base_url, path, body):
        if path != "search":
            raise AssertionError(f"unexpected path: {path}")
        return self.search.get(body["query"], {"items": []})


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


if __name__ == "__main__":
    unittest.main()
