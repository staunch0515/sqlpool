# sqlpool — overall design

Status: draft for review. No application code changes are planned until this is agreed.

## 1. Purpose

A personal desktop tool for running saved, parameterized, **read-only** SQL queries.

The main job: find a saved query in a few keystrokes, fill its parameters in a form, run it, read the rows, and rerun it later with the same or changed values.

It is not a general database client. No schema browser, no writes, no DDL.

## 2. Users and scope

- One user (the author), on macOS. Not shared, no accounts.
- Queries come from the quizbuks repository as `.sql` files with a YAML front matter (see §5).
- Target database: PostgreSQL, read-only.

Out of scope for now: MySQL/SQLite drivers, team sharing, writing data, schema browsing, charts.

## 3. Principles

1. **Files are the source of truth for queries.** The app reads the catalog; it never edits `.sql` files.
2. **Read-only by construction.** Every run is inside `BEGIN READ ONLY … ROLLBACK`, with a statement timeout and a row limit.
3. **Find first, then run.** Search and favorites are the fastest path; everything else is secondary.
4. **Same look as Vioros.** Tokens, spacing, controls and copy follow `docs/ui-style.md`.
5. **Local only.** No network service. Passwords move to the OS keychain.

## 4. Architecture

```
┌──────────────────────────── Tauri window ────────────────────────────┐
│  React + TypeScript (src/)                                           │
│    search · list · favorites · tags · form · results · history       │
│                          │ invoke (JSON)                             │
│  Rust (src-tauri/src/)                                               │
│    catalog.rs  parse front matter, placeholders, literals, lint      │
│    store.rs    SQLite: FTS5 index, favorites, tags, run history      │
│    db.rs       PostgreSQL, read-only transaction, timeout, row cap   │
│    lib.rs      commands                                              │
└──────────────────────────────────────────────────────────────────────┘
        │ reads                    │ reads/writes              │ connects
  catalog folder (.sql)     app data: sqlpool.db        PostgreSQL (read-only)
  source of truth           favorites, tags, history    user's own connection
```

Decisions:
- **Tauri 2 + React + Vite** for the shell. Small binary, native window, Rust for database and file work.
- **SQLite with FTS5 (trigram)** for search. Substring search works for CJK, and the index rebuilds from the catalog in well under a second for ~400 files.
- **tokio-postgres, simple query protocol.** Parameters are rendered as SQL literals. It is simple and testable, and the read-only transaction is enforced by PostgreSQL itself.

## 5. Query format (input)

A catalog file:

```sql
/*---
id: qb.sponsor.analytics.trend
description: ...
params:
  from: { kind: { type: temporal }, required: true }
  bucket: { kind: { type: text, maxLength: 4 }, required: true }
---*/
SELECT ... {{QbStatsRun.zone}} ... WHERE x >= :from
```

- Front matter: `id`, `description`, `params` (order = form order), `results`, `list`, `permissions`.
- Body: `:name` parameters, `{{Entity.field}}` placeholders.
- Placeholders resolve through a mapping JSON generated from the Java entity definitions (`tools/gen_mapping.py`).

Known gaps in the input:
- Temporal entities (`eb.temporal`, e.g. `SysParam`) read the latest version in effect at `timeSlice.asOf` (default now), without tombstones. Implemented in `catalog::temporal_source`; tested against PostgreSQL.
- Platform row scoping (sponsor datasets) is not reproduced. Results are unscoped. Acceptable only because the tool is for the database owner.

## 6. Functional requirements

| # | Requirement | Status |
|---|---|---|
| F1 | Index the catalog folder; reindex on demand and at start | done |
| F2 | Full-text search over id, description, tags, SQL body; 3+ character terms use FTS, shorter use LIKE | done |
| F3 | Favorites; favorites sort first | done |
| F4 | Tags, editable per query, searchable | done |
| F5 | Generated parameter form: type-aware inputs, enums, required markers, empty = NULL | done |
| F6 | Run read-only with timeout and row cap; show rows, count, time | done |
| F7 | Run history per query (last 200); reuse values from a past run | done |
| F8 | Lint: undeclared / unused parameters, unmapped placeholders | done |
| F9 | Connection URL in the OS keychain | **todo** |
| F10 | Settings (catalog folder, mapping file, connection) in the app, not only in localStorage | **todo** |
| F11 | Keyboard: ⌘K search, ↑/↓ to move in the list, Enter to open, ⌘Enter to run | **todo** |
| F12 | Copy rows / copy SQL / export CSV of results | **todo** |
| F13 | SQL editor (Monaco) for viewing and temporary edits, not saved to the file | later |
| F14 | Multiple connections (dev, prod-readonly) | later |

## 7. Screens

One window, three regions. The Vioros layout language: column heads 42px, cards 10px radius, muted secondary text.

```
┌───────────────────────────────────────────────────────────────────────┐
│ ┌──────── Queries (sidebar, 300px) ─┐ ┌──── Detail (main) ──────────┐ │
│ │ [ Search…                  ⌘K ]   │ │ qb.admin.fee-credit-grants  │ │
│ │ 📋 Queries     24 / 366  Reindex   │ │ description…               │ │
│ │ ───────────────────────────────── │ │ permissions · tags · ★      │ │
│ │ ★ qb.admin.fee-credit-summary     │ │ ┌ Parameters (card) ──────┐ │ │
│ │   description…          ran 3×    │ │ │ from *   [datetime]     │ │ │
│ │ ◦ qb.admin.fee-invoices           │ │ │ bucket * [HOUR ▾]       │ │ │
│ │   #weekly #ops                    │ │ │ [Run (read-only)]       │ │ │
│ │                                   │ │ └─────────────────────────┘ │ │
│ │                                   │ │ ⚠ lint (warn banner)        │ │
│ │                                   │ │ Results (table, 1000 max)   │ │
│ │                                   │ │ 🕘 Recent runs  [Reuse]     │ │
│ │                                   │ │ 📄 SQL body (collapsed)     │ │
│ └───────────────────────────────────┘ └─────────────────────────────┘ │
│ Settings: folder · mapping · connection (card, top of main or modal) │
└───────────────────────────────────────────────────────────────────────┘
```

Regions:
- **Sidebar** (`.sidebar`, 300px fixed): search, count, reindex, list. The list scrolls inside the sidebar; nothing else scrolls with it.
- **Main** (`.main`): scrolls as one column. Settings is a card at the top, collapsed after first setup.
- **Below 1100px** (the Vioros breakpoint): the sidebar moves above the main area with a fixed 40vh height.

Window: default 1200×800, minimum 900×600.

States:
- No catalog found → empty state with the folder path and a "Choose folder" hint.
- Parse errors → warn banner in the sidebar with the count; each skipped file listed.
- Run error → error banner in the results area; the failed run stays in history.
- Running → button shows "Running…" and is disabled.
- Missing connection → Run is disabled, the connection field is highlighted.

## 8. Interaction flows

1. **Find and run:** type in search → list filters live (FTS) → ↑/↓ and Enter opens the query → fill form → Run → results appear → history updates.
2. **Favorite:** click ☆ in list or header. Favorites move to the top.
3. **Tag:** edit the tags field; saved on blur or Enter; searchable at once.
4. **Reuse:** click "Reuse values" on a past run; the form refills (temporal values converted back to local time).
5. **Reindex:** click Reindex after the catalog changes; favorites, tags and history stay.

## 9. Visual system

Same as Vioros; full rules in `docs/ui-style.md`. Key points:
- Tokens: `--bg --surface --sidebar --text --muted --border --accent --danger --warn --ok --row-hover`, light and dark.
- Accent `#0a84ff` for primary actions and the active row.
- Banners for errors, warnings and success. No `alert()`.
- Copy: English, sentence case, verb-first buttons; one emoji per section title.

## 10. Data

Local SQLite, `~/Library/Application Support/com.sqlpool.app/sqlpool.db`:

- `query_meta(query_id PK, path, description, tags, favorite, last_run_at, run_count)`
- `query_fts` FTS5 (trigram) over `query_id, description, tags, body`
- `run_history(id, query_id, params_json, ran_at, ok, rows, elapsed_ms, error)`, last 200 per query

Rules:
- Catalog sync rebuilds `query_fts` and keeps `query_meta` and `run_history`.
- A query whose file is gone leaves search but keeps its history.
- `params_json` stores parameter values. This is a personal tool; the file is local and not shared.

## 11. Commands (backend contract)

| command | input | output |
|---|---|---|
| `sync_catalog` | folder | `{indexed, skipped[]}` |
| `search_queries` | text | `QueryHit[]` |
| `set_query_meta` | query_id, favorite?, tags? | — |
| `query_history` | query_id, limit | `RunRecord[]` |
| `get_query` | path, mapping? | front matter, body, lint |
| `run_query` | path, mapping?, conn, params, timeout, max_rows | columns, rows, truncated, elapsed |

Each run is recorded, whether it succeeded or failed.

## 12. Quality and testing

- Rust unit tests: front matter, placeholder resolution, tokenizer (strings, comments, `::`), literal rendering, lint, sync/search/favorites/history (in-memory SQLite).
- Rust integration: a real PostgreSQL (Docker, port 55432) runs the real `analytics-trend.sql` end to end, plus read-only and timeout checks. Gated by `SQLPOOL_TEST_PG`.
- Frontend: `tsc` and `vite build` must pass. Visual check in light and dark, desktop and narrow width.
- Before each release: run the app against a throwaway database, not the production one.

## 13. Roadmap

1. **Now (finish the base):** F9 keychain, F10 settings in app, F11 keyboard. The layout must be verified in the real window, not only in the browser pane.
2. **Next:** F12 copy/export, polish of the list and detail states.
3. **Later:** F13 editor, F14 multiple connections; temporal entity subqueries; mapping generation as a button.

Acceptance for step 1: the app opens at 1200×800 with no overlapping regions; a query can be found, run, favorited, tagged and rerun from history; the connection string is not in localStorage.

## 14. Risks and open questions

- **Layout verification.** Several layout bugs were only visible in the real window (grid tracks growing with long text). Each layout change needs a check in the real window at default and minimum sizes.
- **Mapping drift.** The mapping is generated from Java sources. It must be regenerated when entities change; a stale mapping gives wrong table names silently.
- **Row scoping.** Results are unscoped. Fine for the owner; never use this tool to show data to someone else.
- **Keychain.** Needs a Rust keychain crate and a settings screen; the design needs a decision on what happens when the Keychain is locked.
- Open: should the mapping be regenerated inside the app, or kept as a script run by hand?
- Open: is the sidebar width fixed at 300px enough, or should it become resizable like Vioros?
