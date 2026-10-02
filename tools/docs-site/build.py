#!/usr/bin/env python3
"""Build the Mintlify site from docs/ into an output directory.

docs/ stays the only copy of the documentation. It is written for GitHub,
where links are relative and end in `.md`; Mintlify wants root-relative links
without the extension and refuses relative ones in production. So this copies
the pages docs/docs.json publishes and rewrites each link: to another published
page as `/path`, and to anything else in the repository (source files, the
internal notes .mintignore keeps off the site) as a GitHub URL.

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
        open(dest, "w").write("\n".join(lines))
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
