# Typenx Season Centralizer Addon

This addon sits in front of the MAL, AniList, and Kitsu metadata addons and presents split seasons as one centralized show.

For example, if upstream search returns `Attack on Titan`, `Attack on Titan Season 2`, and `Attack on Titan 3rd Season`, this addon returns one `Attack on Titan` result with all source season entries attached. Opening that result fetches each upstream season and merges the metadata into one show with season-numbered episodes.

## Run

```powershell
$env:PYTHONPATH="../typenx-addon-python-sdk/src;src"
$env:PORT="8790"
python -m typenx_addon_season_centralizer
```

By default it reads from:

- `http://127.0.0.1:8787`
- `http://127.0.0.1:8788`
- `http://127.0.0.1:8789`

Override this with `TYPENX_SEASON_SOURCES`:

```powershell
$env:TYPENX_SEASON_SOURCES="http://127.0.0.1:8787,http://127.0.0.1:8788,http://127.0.0.1:8789"
```
