import unittest

from typenx_addon_season_centralizer.addon import (
    Source,
    centralize_source_previews,
    decode_refs,
)


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


if __name__ == "__main__":
    unittest.main()
