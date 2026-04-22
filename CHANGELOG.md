# Changelog

## 0.1.0

- Rename **episodes** to **runs** (SurrealDB `run` table, `POST /hifz/runs`, `GET /hifz/run/:id`, MCP `hifz_runs`, commit field `run_id`, web UI `/runs`)
- Fix run search: wildcard queries now use plain SELECT instead of BM25
- Add runs and commits to export endpoint (`runs` key in export JSON)
- Fix type::thing -> type::record migration for SurrealDB 3.x

