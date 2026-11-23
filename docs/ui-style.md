# UI style

sqlpool uses the same visual standard as the Vioros app, so both look like one team's work.
This file is the single source for styling. Every style change follows it.
The rules live in `src/App.css`. Components use class names and tokens, not hard-coded colors.

## 1. Layout

Same shell as the Vioros app:

- Top bar (44px): app name on the left, global search in the middle (420px), settings icon on the right.
- Body row: left navigation column (190px, `.nav-col`) and main column (`.main-col`).
- Navigation: a list of pages (`.nav-item`, active = accent fill, white text). Each page is one tab.
- Current-path bar (`.col-head.path-bar`, 42px): "📍 Current: <page>".
- Left column under the navigation: the groups of the current list (`.nav-groups`), "All" first, each with its count. Choosing a group filters the list. Groups belong to the left column, not to the table.
- Every list page (Catalog, Saved SQL, History) has the same parts, in this order: toolbar (`.table-toolbar`: category tabs on the left, page actions on the right), table, action bar for the selected row, detail on the left with info panel on the right.
- List and detail (`.list-detail`), used by the Catalog, Saved SQL and History pages:
  - a table on top (`.data-table`, 40% height), rows select on click (`tr.selected`);
  - below it, the detail on the left (`.detail-main`, collapsible sections) and an info panel on the right (`.info-panel`, 280px, `.info-rows`).
- Status bar (26px, `.status-bar`): connection dot and text on the left, read-only mark on the right.
- Collapsible blocks (`.section`, a `<details>` element) are closed by default unless they hold what the user needs now (parameters, results, warnings).
- Settings is a page of its own (`.settings-page`), reached from the gear icon.
- Responsive: one breakpoint at `max-width: 1100px`. Below it the detail stacks above the info panel.

### 1a. Dragging (Vioros behaviour, copied exactly)

- Every divider is draggable: the navigation column, the table/detail divider, and the info panel divider (`.splitter-v`, `.splitter-h`, 8px hit area, muted, accent on hover).
- Drag handling: pointer events on the divider, move and up listeners on `window`, sizes clamped to a range, saved to localStorage on release. The body gets `resizing` (column cursor) or `resizing-y` (row cursor) during the drag, and text selection is blocked. Code: `src/resize.tsx`.
- Ranges used: navigation 160–640px (Vioros tree), info panel 220–800px, table height 120–900px.
- Tables: click a header to sort ascending, click again for descending, a third time clears the sort. The header shows ▲ or ▼. Columns with no sort value are not sortable.
- Column widths: drag the right edge of a header (8px area at the header's right edge, accent on hover). Columns share the table width in proportion to their widths, so the table always fills its box. Minimum column width 60px. Widths are saved per list.

## 2. Spacing and sizes

- Column head height 42px, horizontal padding 16px.
- Main padding 16px 20px. Card `.card`: padding 14px, gap 12px, radius 10px.
- Radius: buttons, inputs, list rows, `.banner` 6–8px; cards 10px; table wrap and SQL block 8px.
- Form grid gap 12px. Field labels `.field` and hints `.hint`: 13px, muted.

## 3. Colors: tokens only

Defined on `:root` in `App.css`, the same names and values as Vioros:

| token | light | dark | use |
|---|---|---|---|
| `--bg` | #f5f5f7 | #1c1c1e | page background |
| `--surface` | #ffffff | #2c2c2e | cards, inputs, buttons (ghost) |
| `--sidebar` | #ececf0 | #242426 | sidebar, table header, SQL block |
| `--text` | #1d1d1f | #f5f5f7 | body text |
| `--muted` | #6e6e73 | #98989d | secondary text |
| `--border` | #d2d2d7 | #3a3a3c | borders and dividers |
| `--accent` | #0a84ff | (same) | primary button, active row, focus border |
| `--danger` | #ff3b30 | (same) | errors |
| `--warn` | #ff9f0a | (same) | warnings, favorite star |
| `--ok` | #34c759 | (same) | success |
| `--row-hover` | rgba(127,127,127,.08) | rgba(255,255,255,.06) | hover on rows |

- Dark mode is defined in both places: `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }` and `:root[data-theme="dark"]`.
- New colors are not added to components. Use a token.

### Allowed exceptions

- `#fff` on accent: text on a primary button and on the active list row (`.item.active`).
- Banner tints: `rgba()` of `--danger`, `--warn`, `--ok` at 12–14% opacity (`.banner.error`, `.banner.warn`, `.banner.ok`).

## 4. Typography

- System font stack as in Vioros. Base 14px, line-height 1.5.
- Page title `.page-title`: 18px / 600. Column and card titles `.card-title`: 14px / 600.
- Secondary text `.page-sub`, `.meta`, `.hint`, `.item-desc`: 13px, muted.
- Code and SQL: `.mono` or `code`, 13px, monospace.

## 5. Controls

- Default `button`: accent background, white text, radius 6px, padding 6px 10px, weight 500.
- `button.ghost` and `button.small`: surface background, text color, border. `small` is 3px 9px, 13px.
- `button.crumb`: text-style link in accent, no background. Used for "Reindex".
- Inputs, selects, buttons: 6px radius, 1px border, surface background. Focus: border becomes `--accent`.
- Disabled: `opacity: .5`, default cursor.
- Favorites: `.star` (inline) and `.star.big` (in the header). Favorite color is `--warn`.

## 6. Status and messages

- Errors: `.banner.error`. Warnings and parse issues: `.banner.warn`. Success: `.banner.ok`.
- Do not use `alert()`.
- Empty state `.empty`: centered, muted, 36px vertical padding.

## 7. Copy

- English, sentence case. Button labels start with a verb: "Run (read-only)", "Reindex", "Reuse values".
- Page names in the navigation carry one emoji each: 📋 Catalog, 📄 Saved SQL, 🕘 History.
- Placeholders show an example value (e.g. `postgres://user:pass@host:5432/db`).

## 8. Known differences from Vioros

- No toast component yet. sqlpool shows state with banners only.
- The table on the Catalog page is not yet a Vioros-style tree; it is a flat list of queries.

## 9. Checks before a style change lands

1. `pnpm build` passes.
2. No new hex colors or `rgba()` outside `:root` and the exceptions above (`grep -n "#[0-9a-fA-F]\{3,6\}\|rgba" src/App.css`).
3. Look at light and dark mode, and at a window narrower than 1100px.
4. Behavior unchanged: search, favorites, tags, run, history reuse.
