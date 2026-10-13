# Memhot private search

This optional SearXNG service aggregates free public search results. It is a discovery source, not a guarantee of factual accuracy, freshness, complete coverage, or continued engine availability.

## Running deployment

- Private endpoint: `http://127.0.0.1:8791/search` (POST, JSON).
- Memhot operator configuration: `MEMHOT_SEARCH_URL=http://127.0.0.1:8791`.
- Container: `memhot-search`; working directory: `/opt/memhot-search`.
- Image: `docker.io/searxng/searxng@sha256:ddd642cae6eac75412938fd846ddbf440a2df029cfeae8a562b8b205f4e35e25`.
- Observed version: `2026.10.11-ad46ebd7d`. Pin updates to an independently checked digest.
- Memory ceiling 256 MiB, no extra swap, 0.5 CPU, one worker and two blocking threads, 64 processes. No additional Redis/Valkey service.
- Port binds only IPv4 loopback. Do not add a public reverse proxy or change it to `0.0.0.0`.
- Query access logging is disabled; Docker uses the `none` logging driver. Debug and metrics are disabled. Public engine providers still receive the search query, so private notes must never be included in shared public discovery queries.
- Two engines enabled: DuckDuckGo and 360search. Captchas or blocked engines remain visible in `unresponsive_engines`; the service does not bypass them.

The deployment did not change `memhot.service` or stop any pre-existing service. Enabling the Rust search adapter is a separate Memhot release step. The adapter should use a fixed operator-controlled loopback endpoint, no proxy, no redirects, response-size bounds, a deadline and a concurrency gate; never relax ordinary external URL SSRF checks.

## Install / update

Reuse an existing Docker installation. Inspect host RAM, swap, disk and listening ports first. Do not prune unrelated images, stop unrelated containers or alter their configuration.

Copy `compose.yaml`, `settings.template.yml` and `benchmark.py` to a private directory. The pinned image uses UID/GID 977. Create `config` and `data` owned by 977:977 with mode 0700. Generate a random `server.secret_key` on the server, replace `GENERATE_ON_DEPLOY` in `config/settings.yml`, and make it 0600. Never commit that generated file. Keep the directory containing it 0700.

```
cd /opt/memhot-search
docker compose config --quiet
docker compose up -d
curl --fail http://127.0.0.1:8791/healthz
docker stats --no-stream memhot-search
python3 benchmark.py --combined --auto-language --output benchmark-final.json
python3 benchmark.py --combined --auto-language --recent --output benchmark-recent.json
```

Do not print resolved compose configuration or private settings into public logs. To pause only this service, run `docker compose stop search` in its directory. `docker compose start search` resumes it. A digest update requires rerunning the synthetic query checks: search engine availability is independent of container health.

## Synthetic validation, 2026-10-12

Public queries and results are in `artifacts/memhot/search/`; none contain user queries or credentials. Only titles, URLs, engine names and snippet lengths are stored.

- DuckDuckGo initially returned useful English SQLite WAL and business cash-flow results, including the official SQLite documentation. Chinese requests encountered CAPTCHA; later aggregate checks reported the same engine unavailable. This provider is not dependable from the current host.
- 360search returned results for all four English/Chinese queries. The Chinese CPI query included official `stats.gov.cn` explanation pages. English SQLite results were mostly Chinese technical explanations. Business cash-flow results were commercially skewed. The learning query mixed commentary, a university journal article and catalog/listing pages.
- Bing returned English responses but sometimes discarded the query intent (dictionary results for the word “small”), and no Chinese results; it was removed.
- Brave returned HTTP 429, Baidu CAPTCHA, Google access denial, and Sogou an engine error. Wikipedia returned no results for these broad multi-word queries. They were removed. No login, paywall, CAPTCHA or anti-bot bypass was attempted.
- Final two-engine aggregate checks returned 4–6 results per query without a time filter and 5–7 with `time_range=month`, about 1.6–3.7 seconds. A requested month filter does not prove every returned item is recent: the article date must still be checked by Memhot's freshness validation.

A health check proves the local service responds. HTTP 200 or a nonzero count does not prove relevance. Low-quality, undated or inaccessible sources must remain excluded from publication or explicitly marked as gaps. Public editions should combine vetted feeds with search and retain independent editorial/citation checks.

## Official references

- [Official container installation](https://docs.searxng.org/admin/installation-docker.html)
- [Search settings and JSON format](https://docs.searxng.org/admin/settings/settings_search.html)
- [Server settings](https://docs.searxng.org/admin/settings/settings_server.html)
- [Engine configuration](https://docs.searxng.org/admin/settings/settings_engines.html)
- [Granian service options](https://docs.searxng.org/admin/installation-granian.html)
