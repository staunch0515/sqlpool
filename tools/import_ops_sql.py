#!/usr/bin/env python3
"""Pulls the read-only SQL snippets out of a Markdown document (the QuizBuks ops manual), for the Saved SQL
folders of sqlpool. Writes a JSON list: name, group, description, sql. Placeholders become :parameters:
'<转账号>' -> :transferRef, '…' -> :value, and so on.

Usage: tools/import_ops_sql.py path/to/operations.md > items.json
"""
import json
import re
import sys

# Known placeholders from the manual, by their text, to parameter names.
KNOWN = {
    "转账号": "transferRef",
    "学者": "userId",
    "发布": "publicationId",
    "编号": "sharedVideoId",
    "发起方": "sponsorId",
}

CODE = re.compile(r"`([^`]+)`")
SQL_START = re.compile(r"^\s*(SELECT|WITH)\b", re.I)


def clean(text):
    text = re.sub(r"\*\*|__", "", text)
    text = re.sub(r"^\s*[-*]\s+", "", text)
    text = re.sub(r"^\s*\d+\.\s+", "", text)
    return text.strip(" ：:。；;，,")


def parameterize(sql):
    counter = {"n": 0}

    def named(m):
        label = m.group(1)
        name = KNOWN.get(label)
        if name is None:
            counter["n"] += 1
            name = f"value{counter['n']}"
        return f":{name}"

    # '<label>' inside quotes becomes the parameter itself (the quotes go too)
    sql = re.sub(r"'<([^>]+)>'", named, sql)
    # a bare … is a value still to fill in
    while "…" in sql:
        counter["n"] += 1
        sql = sql.replace("…", f":value{counter['n']}", 1)
    return sql


def extract(path):
    lines = open(path, encoding="utf-8").read().split("\n")
    items = []
    h2 = h3 = ""
    in_fence = False
    fence_buf = []
    fence_line = 0
    for no, line in enumerate(lines, start=1):
        if line.strip().startswith("```"):
            if in_fence:
                items.extend(fenced_statements(h2, h3, fence_buf, fence_line))
                fence_buf = []
                in_fence = False
            else:
                in_fence = True
                fence_line = no
            continue
        if in_fence:
            fence_buf.append(line)
            continue
        if line.startswith("## "):
            h2, h3 = line[3:].strip(), ""
            continue
        if line.startswith("### "):
            h3 = line[4:].strip()
            continue
        for m in CODE.finditer(line):
            code = m.group(1).strip()
            if not SQL_START.match(code):
                continue
            prefix = clean(re.sub(CODE, "", line[: m.start()]))
            items.append(make(h2, h3, code, no, prefix or h3, False, full=clean(re.sub(CODE, "", line))))
    return items


def short_name(label, group):
    """The first clause of the line before the code, e.g. '学者的设置' from '学者的设置：`SELECT …`'."""
    piece = re.split(r"[：:。；;]", label)[0].strip()[:40]
    if len(piece) < 4:
        head = re.sub(r"（.*?）|\(.*?\)", "", group).strip()[:30]
        piece = f"{head} · {piece}" if piece else head
    return piece or group[:40]


def fenced_statements(h2, h3, lines, start_line):
    """Each statement of a fenced block. A `-- comment` line before it names it; the statement ends at a line
    ending in `;`. Blocks that are not SQL (no SELECT or WITH) give nothing."""
    out = []
    comment = ""
    chunk = []
    line_no = start_line
    for offset, raw in enumerate(lines, start=1):
        line = raw.rstrip()
        stripped = line.strip()
        if not chunk and stripped.startswith("--"):
            comment = stripped.lstrip("-").strip()
            continue
        if not chunk and not stripped:
            continue
        if not chunk:
            line_no = start_line + offset
        chunk.append(line)
        if stripped.endswith(";"):
            sql = "\n".join(chunk).rstrip().rstrip(";").strip()
            if SQL_START.match(sql):
                out.append(make(h2, h3, sql, line_no, comment, True, full=comment))
            chunk = []
            comment = ""
    if chunk:
        sql = "\n".join(chunk).strip()
        if SQL_START.match(sql):
            out.append(make(h2, h3, sql, line_no, comment, True, full=comment))
    return out


def make(h2, h3, sql, line_no, label, fenced, full=""):
    group = h3 or h2 or "ops"
    name = short_name(label, group)
    sql = parameterize(sql.strip())
    description = (full or label or group)[:300]
    return {
        "name": name,
        "group": group,
        "description": f"{description} (operations.md line {line_no})",
        "sql": sql,
        "fenced": fenced,
    }


if __name__ == "__main__":
    items = extract(sys.argv[1])
    json.dump(items, sys.stdout, ensure_ascii=False, indent=1)
    print(file=sys.stderr)
    print(f"{len(items)} statements", file=sys.stderr)
