# Typenx Season Centralizer Addon

Collapse split-season anime releases into one centralized show.

Anime metadata providers don't agree on what counts as a "show." MyAnimeList, AniList, and Kitsu all tend to split long-running anime into separate records: *Attack on Titan*, *Attack on Titan Season 2*, *Attack on Titan: The Final Season*, and so on — each with its own poster, its own episode numbering, its own ID. That makes any library built directly on top of those providers feel scattered.

This addon sits between the metadata addons and [Typenx Core](https://github.com/typenx/typenx-core) and presents those split releases as one show with merged season-numbered episodes. Search for *Attack on Titan* and you get one result; open it and you get a unified episode list across all the source seasons.

## Run

```powershell
$env:PORT="8790"
cargo run
```

By default it reads from the official addon ports:

- `http://127.0.0.1:8787` — MyAnimeList
- `http://127.0.0.1:8788` — AniList
- `http://127.0.0.1:8789` — Kitsu

Override with `TYPENX_SEASON_SOURCES`:

```powershell
$env:TYPENX_SEASON_SOURCES="http://127.0.0.1:8787,http://127.0.0.1:8788,http://127.0.0.1:8789"
```

## Episode thumbnails

Season Centralizer enriches missing episode thumbnails from TVMaze episode images when available. Disable that external lookup with:

```powershell
$env:TYPENX_TVMAZE_EPISODE_IMAGES="0"
```

## Why this is its own addon

The centralization logic could live inside each metadata addon, but it shouldn't — different users want different behavior (some prefer per-season cards, some don't), and the merge logic relies on cross-provider matching that gets cleaner with more sources, not fewer. Making it its own addon keeps the metadata addons honest and lets the centralizer evolve on its own cadence.
