#!/usr/bin/env python3
"""Build the Mintlify site from docs/ into an output directory.

docs/ stays the only copy of the documentation. It is written for GitHub,
where links are relative and end in `.md`; Mintlify wants root-relative links
without the extension and refuses relative ones in production. So this copies
the pages docs/docs.json publishes and rewrites each link: to another published
page as `/path`, and to anything else in the repository (source files, the
internal notes .mintignore keeps off the site) as a GitHub URL.

Pages are plain Markdown that GitHub renders natively; components come from
syntax GitHub already understands, translated here, so nothing is written
twice:

  > [!NOTE] / [!TIP] / [!WARNING] / [!CAUTION] / [!IMPORTANT]
                                   -> <Note> <Tip> <Warning> <Danger> <Info>
  <details><summary>T</summary>    -> <Accordion title="T">
  <!-- steps --> ... <!-- /steps -->  with a heading per step -> <Steps>
  <!-- tabs --> ... <!-- /tabs -->    with a heading per tab  -> <Tabs>
  <!-- cards --> ... <!-- /cards -->  "- [Title](link): text" -> <Card>s
  # Title, <!-- description: ... -->, <!-- icon: ... -->,
  <!-- sidebarTitle: ... -->            -> frontmatter

Any other HTML comment is dropped, since MDX does not accept them.

Usage: build.py <out-dir>
"""
import json
import os
import re
import shutil
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DOCS = os.path.join(REPO, "docs")
GITHUB = "https://github.com/luohoa97/cordial/blob/main/"
LINK = re.compile(r"(!?\[[^\]]*\]\()([^)\s]+)(\))")
FENCE = re.compile(r"^\s*(```|~~~)")


def published_pages():
    cfg = json.load(open(os.path.join(DOCS, "docs.json")))
    pages = set()

    def walk(node):
        if isinstance(node, dict):
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                if isinstance(v, str):
                    pages.add(v)
                else:
                    walk(v)

    walk(cfg["navigation"])
    return pages


def page_file(page):
    for ext in (".mdx", ".md"):
        if os.path.exists(os.path.join(DOCS, page + ext)):
            return page + ext
    raise SystemExit(f"docs.json names {page}, which has no .md or .mdx file")


def rewrite(target, src_rel, pages, assets):
    if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith(("#", "/")):
        return target
    path, _, anchor = target.partition("#")
    anchor = "#" + anchor if anchor else ""
    joined = os.path.normpath(os.path.join(os.path.dirname(src_rel), path))
    if joined.startswith(".."):
        return GITHUB + os.path.normpath(os.path.join("docs", joined)) + anchor
    stem, ext = os.path.splitext(joined)
    if ext in (".md", ".mdx") and stem in pages:
        return "/" + stem + anchor
    if ext in (".md", ".mdx") or not os.path.isfile(os.path.join(DOCS, joined)):
        return GITHUB + "docs/" + joined + anchor
    assets.add(joined)
    return "/" + joined + anchor


ALERTS = {"NOTE": "Note", "TIP": "Tip", "WARNING": "Warning", "CAUTION": "Danger", "IMPORTANT": "Info"}
META = re.compile(r"^<!--\s*(description|icon|sidebarTitle)\s*:\s*(.*?)\s*-->\s*$")
REGION = re.compile(r"^<!--\s*(/?)(steps|tabs|cards)\s*-->\s*$")
HEADING = re.compile(r"^(#{2,6})\s+(.*)$")
CARD = re.compile(r"^\s*[-*]\s+\[([^\]]+)\]\(([^)]+)\)\s*[:\u2014-]?\s*(.*)$")


def attr(text):
    return text.replace("\\", "\\\\").replace('"', '\\"')


def region(kind, body):
    if kind == "cards":
        cards = [CARD.match(l) for l in body if CARD.match(l)]
        out = ["<Columns cols={2}>"]
        for m in cards:
            out += [f'  <Card title="{attr(m.group(1))}" href="{m.group(2)}">', f"    {m.group(3)}", "  </Card>"]
        return out + ["</Columns>"]
    outer, inner = ("Steps", "Step") if kind == "steps" else ("Tabs", "Tab")
    level = next((len(m.group(1)) for m in map(HEADING.match, body) if m), None)
    out, open_ = [f"<{outer}>"], False
    for l in body:
        m = HEADING.match(l)
        if m and len(m.group(1)) == level:
            if open_:
                out += ["", f"</{inner}>"]
            out += [f'<{inner} title="{attr(m.group(2))}">', ""]
            open_ = True
        elif open_:
            out.append(l)
    if open_:
        out += ["", f"</{inner}>"]
    return out + [f"</{outer}>"]


def components(lines, is_md):
    meta, out, i, infence = {}, [], 0, False
    while i < len(lines):
        l = lines[i]
        if FENCE.match(l):
            infence = not infence
            out.append(l)
            i += 1
            continue
        if infence:
            out.append(l)
            i += 1
            continue
        if is_md and "title" not in meta and l.startswith("# "):
            meta["title"] = l[2:].strip()
            i += 1
            continue
        m = META.match(l)
        if m:
            meta[m.group(1)] = m.group(2)
            i += 1
            continue
        m = re.match(r"^>\s*\[!(\w+)\]\s*$", l)
        if m and m.group(1).upper() in ALERTS:
            tag = ALERTS[m.group(1).upper()]
            body = []
            i += 1
            while i < len(lines) and lines[i].startswith(">"):
                body.append(re.sub(r"^>\s?", "", lines[i]))
                i += 1
            out += [f"<{tag}>", ""] + body + ["", f"</{tag}>"]
            continue
        m = REGION.match(l)
        if m and not m.group(1):
            kind, body = m.group(2), []
            i += 1
            while i < len(lines) and not REGION.match(lines[i]):
                body.append(lines[i])
                i += 1
            i += 1
            out += region(kind, body)
            continue
        m = re.match(r"^\s*<details>\s*(<summary>(.*?)</summary>)?\s*$", l)
        if m:
            title = m.group(2)
            if title is None and i + 1 < len(lines):
                s = re.match(r"^\s*<summary>(.*?)</summary>\s*$", lines[i + 1])
                if s:
                    title, i = s.group(1), i + 1
            out += [f'<Accordion title="{attr(title or "Details")}">', ""]
            i += 1
            continue
        if re.match(r"^\s*</details>\s*$", l):
            out += ["", "</Accordion>"]
            i += 1
            continue
        out.append(l)
        i += 1
    text = re.sub(r"<!--.*?-->", "", "\n".join(out), flags=re.S)
    if meta:
        front = ["---"] + [f'{k}: "{attr(v)}"' for k, v in meta.items()] + ["---", ""]
        text = "\n".join(front) + text.lstrip("\n")
    return text


def main():
    out = sys.argv[1]
    if os.path.exists(out):
        shutil.rmtree(out)
    os.makedirs(out)
    pages = published_pages()
    assets = set()
    for page in sorted(pages):
        rel = page_file(page)
        lines = open(os.path.join(DOCS, rel)).read().split("\n")
        infence = False
        for i, line in enumerate(lines):
            if FENCE.match(line):
                infence = not infence
            elif not infence:
                lines[i] = LINK.sub(lambda m: m.group(1) + rewrite(m.group(2), rel, pages, assets) + m.group(3), line)
        dest = os.path.join(out, rel)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        has_front = bool(lines) and lines[0].strip() == "---"
        open(dest, "w").write("\n".join(lines) if has_front else components(lines, rel.endswith(".md")))
    shutil.copy(os.path.join(DOCS, "docs.json"), out)
    for extra in ["logo"]:
        shutil.copytree(os.path.join(DOCS, extra), os.path.join(out, extra))
    for asset in sorted(assets):
        dest = os.path.join(out, asset)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copy(os.path.join(DOCS, asset), dest)
    print(f"{len(pages)} pages, {len(assets)} assets -> {out}")


if __name__ == "__main__":
    main()
