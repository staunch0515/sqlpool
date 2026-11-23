# sqlpool — project notes for Claude

A personal desktop client for running saved, parameterized, **read-only** SQL queries.
The main job: find a saved query quickly, fill its parameters in a form, run it, and see the rows.
It is not a general-purpose DB client (no schema browser, no writes).

## Stack (decided)

- Tauri 2, React 19 + TypeScript + Vite (frontend in `src/`).
- Rust backend in `src-tauri/src/`, using `tokio-postgres` for PostgreSQL, `serde_yaml_ng` for front matter, `indexmap` to keep param order.
- Target DB: **PostgreSQL only** for now. MySQL and SQLite are not started.
- Queries are read-only. Execution always runs in `BEGIN READ ONLY` … `ROLLBACK`.

## Commands

```bash
pnpm install
pnpm tauri dev                 # run the desktop app
pnpm build                     # tsc + vite build
cd src-tauri && cargo test     # unit tests (the real-file test skips if the repo is absent)
cd src-tauri && cargo clippy --all-targets
```

PostgreSQL integration test (skipped unless the env var is set):

```bash
docker run -d --name sqlpool-pg -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=test -p 55432:5432 postgres:16-alpine
SQLPOOL_TEST_PG="host=localhost port=55432 user=postgres password=pw dbname=test" cargo test
```

## Query file format (from the quizbuks repo)

Catalog root: `/Users/zhongpingxue/quizbuks/repo_quizbuks/backend/quizbuks/src/main/resources/queries/`

- A file starts with a YAML front matter inside `/*--- … ---*/`: `id`, `description`, `entities`, `datasets`, `params`, `results`, `list`, `permissions`.
- Params: `{ like, kind: {type, maxLength, enum, …}, required, description }`. Declaration order is the form order.
- Body placeholders: `{{Entity}}` and `{{Entity.field}}`. They are resolved through a JSON mapping file (`"Entity.field"` first, then `"Entity"`). Without a mapping, these queries cannot run.
- Params are `:name`. Rendered as SQL literals (`'…'` with `''` escaping; empty → `NULL`), not bound with driver bind types. Both `::` casts and `:` inside strings or comments are ignored by the tokenizer (`catalog::tokenize`).
- Temporal form values are treated as UTC.

## Code map

- `src-tauri/src/catalog.rs` — front matter parsing, placeholder resolution, tokenizer, literal rendering, lint.
- `src-tauri/src/db.rs` — read-only execution, row limit, statement timeout, server error messages (`pg_error`).
- `src-tauri/src/store.rs` — local SQLite store in the app data dir (`sqlpool.db`): FTS5 index over the catalog (`query_fts`, trigram tokenizer), `query_meta` (favorite, tags, last run, run count), `run_history` (last 200 runs per query). Catalog files stay the source of truth; sync rebuilds the index and keeps favorites, tags and history. Removed files drop out of search but keep their history.
- `src-tauri/src/lib.rs` — Tauri commands `sync_catalog`, `search_queries`, `set_query_meta`, `query_history`, `get_query`, `run_query`; `run_query` records every run, successful or not. `RunArgs` is camelCase on the JS side.
- `src-tauri/src/mcp.rs` + `src-tauri/src/bin/sqlpool-mcp.rs` — read-only MCP server for Claude Desktop (stdio). Tools: `search_queries`, `describe_query`, `run_query` (saved catalog queries), `list_tables` (mapped entities and fields), `run_sql` (one read-only SELECT over mapped tables only; the check is `adhoc.rs`, a lexical guard in front of the read-only transaction). Own index and run history in `sqlpool-mcp.db` next to its config `mcp.json`; the password is in the keychain (`sqlpool-mcp --set-password`). No writes, scripts or server commands on purpose. Direct PostgreSQL only (no ssh data sources). Row-level scoping is not replicated, so the database account must be read-only.
- `src/App.tsx` — search (backend FTS), favorites, tags, generated form, result table, recent runs with "reuse", SQL body.

## Decisions and open items

- Settings (catalog dir, mapping path, connection URL) live in localStorage **for now**. Passwords must move to the OS keychain (`keyring` crate) before real use.
- Physical names come from the Java entity definitions: `EntityDefinition.define(CONST, eb -> { eb.physicalTable("t"); eb.field("f", f -> f.physicalColumn("c")) })`, plus helper calls like `count(eb, "views", "views")` and `PublisherKind.declare(...)`.
- `tools/gen_mapping.py` generates the mapping from those definitions: `python3 tools/gen_mapping.py /Users/zhongpingxue/quizbuks/repo_quizbuks/backend > tools/out/mapping.json`. It scans `src/main` only, resolves constants per file (many classes reuse `CARD = "card"`), and prints warnings for temporal entities and for helper fields it matched by name only. Re-run it when the Java entities change.
- Temporal entities (`eb.temporal`, e.g. `SysParam`) are mapping objects, not table names. `{{Entity}}` becomes the platform's "latest version in effect at asOf, without tombstones" subquery (`catalog::temporal_source`, docs/design/04 section 5.1, decision D3). Write-once entities read the rows directly. `timeSlice: {asOf: <param>, knownAt: <param>}` in the front matter chooses the time; without it the read is at `now()`, as the platform does by default. `lint` warns when a temporal entity is read without `timeSlice`.
- The temporal subquery is covered by a unit test and by `db::e2e_tests::temporal_entity_end_to_end` (drops and recreates `sys_param_version`).
- **Row scoping is not replicated.** Platform datasets such as `urn:jabiz:dataset:sponsor:…` restrict rows to the caller. Running SQL against the physical tables returns all rows. This is fine only for the DB owner's own use.
- `e2e_tests::analytics_trend_end_to_end` (in `src-tauri/src/db.rs`) runs the real `analytics-trend.sql` through the mapping against PostgreSQL. It **drops and recreates** `qb_publication_card`, `qb_analytics_hour`, `qb_stats_run` in the `SQLPOOL_TEST_PG` database, so use a throwaway database only.
- Not done yet: Monaco editor, multiple connections, MySQL/SQLite drivers.
- Search: terms of 3+ characters use the trigram FTS index; shorter terms use LIKE. Several terms are ANDed.
- The app's database is at `~/Library/Application Support/com.sqlpool.app/sqlpool.db`. Delete it to reset favorites and history; the index rebuilds from the catalog on start.
- Cmd+K only focuses the search box. It is not a command palette.
- The GUI has not been launched and checked by hand yet.
- Keep the `tokenize` rules in sync with any change to how params are written in the catalog SQL.
