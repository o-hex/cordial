#!/usr/bin/env python3
"""Post a published release to a Discord webhook.

Usage: discord_release.py [--dry-run] RELEASE.json [RELEASES.json]

RELEASE.json is the GitHub API's release object, RELEASES.json the repository's
release list (newest first), used only to find the tag this one follows for the
compare link. The webhook comes from DISCORD_WEBHOOK and the optional mention
from DISCORD_RELEASE_MENTION; neither is ever printed.

This replaced a third-party action that was deleted upstream. What it taught is
kept here because it is still true of Discord: an embed over its limits is
answered with a 400 and nothing is posted, so every limit below is applied
before sending, and any non-2xx answer fails the run with Discord's own error
body. The action logged the status code and carried on, which is how a release
could be too long to announce and still go green.

Cordial's notes are long on purpose -- they carry a what-is-broken section, a
rule in AGENTS.md -- and four of the five releases measured at the time
exceeded the description cap on their own: v0.12.1 at 5,376 characters, v0.12.0
at 6,593, v0.11.0 at 9,657, v0.9.0 at 5,064. The asset list overflows
separately: ten files per release, five packages with a `.cosign.bundle`
signature beside each, came to about 1,600 characters against a 1,024 cap. The
signatures are dropped (they are on the release page, where somebody verifying
a download is already looking) and the rest is trimmed until it fits.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

# Discord's documented caps. The per-embed total is the one that catches
# people out: every individual field can be legal and the embed still rejected.
CONTENT_MAX = 2000
TITLE_MAX = 256
DESCRIPTION_MAX = 4096
FIELDS_MAX = 25
FIELD_NAME_MAX = 256
FIELD_VALUE_MAX = 1024
FOOTER_MAX = 2048
AUTHOR_MAX = 256
EMBED_TOTAL_MAX = 6000

# Left unused against the total, so a miscount here costs a few hundred
# characters of notes rather than the announcement.
SAFETY = 256

USERNAME = "Cordial"
COLOUR = 0x5865F2
FOOTER = "Cordial loads Roblox natively on Linux"

# Discord's edge answers the default Python-urllib agent with a 403.
USER_AGENT = "cordial-release-announcer (https://github.com/luohoa97/cordial)"


def clip(text: str, limit: int) -> str:
    """Cut to `limit` characters, marking the cut."""
    if len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


def close_open_fence(text: str) -> str:
    """Re-close a code fence that the trim cut through.

    Truncating markdown mid-fence turns the rest of the embed into one code
    block in Discord's renderer, which looks like a formatting bug in the
    release notes rather than in the thing that truncated them.
    """
    if text.count("```") % 2 == 1:
        return text + "\n```"
    return text


def trim_body(body: str, budget: int, url: str) -> str:
    """Cut notes to `budget`, at a line boundary, pointing at the full text."""
    if len(body) <= budget:
        return body

    tail = f"\n\n**[Read the full release notes]({url})**"
    room = max(budget - len(tail), 0)

    cut = body[:room]
    # Prefer a paragraph break, then any line break, over slicing a sentence
    # -- or worse, a markdown link -- in half.
    for sep in ("\n\n", "\n"):
        at = cut.rfind(sep)
        # Only honour a boundary that is not throwing most of the notes away.
        if at > room * 0.5:
            cut = cut[:at]
            break

    return close_open_fence(cut.rstrip()) + tail


def source_field(assets: list[dict], repo: str, tag: str) -> str:
    """The Source Code field: package links, then ZIP and TAR, within 1,024.

    Signatures are dropped first, then packages from the end until it fits.
    The ZIP and TAR links always stay; they are the last thing in the field.
    """
    keep = [a for a in assets if not a["name"].endswith(".cosign.bundle")]
    fixed = (
        f"[ZIP](https://github.com/{repo}/zipball/{tag})"
        f" | [TAR](https://github.com/{repo}/tarball/{tag})"
    )

    def render(items: list[dict]) -> str:
        parts = [f"[{a['name']}]({a['browser_download_url']})" for a in items]
        return " | ".join(parts + [fixed])

    while keep and len(render(keep)) > FIELD_VALUE_MAX:
        keep.pop()
    return render(keep)


def previous_tag(releases: list[dict], tag: str) -> str | None:
    """The tag of the release published just before `tag`.

    The list comes back newest-first, so this is the entry after the one being
    announced. Drafts are skipped -- a draft has no tag anybody can compare
    against.

    `None` when there is nothing before it, which is a real case exactly once
    and then never again.
    """
    published = [r for r in releases if not r.get("draft") and r.get("tag_name")]
    for i, r in enumerate(published):
        if r["tag_name"] == tag:
            nxt = published[i + 1 :]
            return nxt[0]["tag_name"] if nxt else None
    # The release is not in the list -- a very long release history and a
    # paginated read, or a tag announced before the API caught up. Falling back
    # to the newest entry that is not this one is better than comparing against
    # a branch.
    for r in published:
        if r["tag_name"] != tag:
            return r["tag_name"]
    return None


def embed_size(embed: dict) -> int:
    """The characters Discord counts against the 6,000 per-embed total."""
    total = len(embed.get("title", "")) + len(embed.get("description", ""))
    total += len(embed.get("footer", {}).get("text", ""))
    total += len(embed.get("author", {}).get("name", ""))
    for f in embed.get("fields", []):
        total += len(f["name"]) + len(f["value"])
    return total


def build_payload(
    release: dict,
    history: list[dict] | None = None,
    mention: str = "",
    repo: str = "luohoa97/cordial",
    now: datetime | None = None,
) -> dict:
    """The webhook body for one release. No I/O."""
    tag = release["tag_name"]
    html_url = release.get("html_url") or f"https://github.com/{repo}/releases/tag/{tag}"
    author = release.get("author") or {}

    content = f"New Release: `{tag}` in `{repo}`"
    if mention:
        content = f"{mention} {content}"

    embed: dict = {
        "title": clip(release.get("name") or tag, TITLE_MAX),
        "url": html_url,
        "color": COLOUR,
        "footer": {"text": clip(FOOTER, FOOTER_MAX)},
        "timestamp": release.get("published_at")
        or (now or datetime.now(timezone.utc)).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }

    if author.get("login"):
        embed["author"] = {"name": clip(author["login"], AUTHOR_MAX)}
        if author.get("html_url"):
            embed["author"]["url"] = author["html_url"]
        if author.get("avatar_url"):
            embed["author"]["icon_url"] = author["avatar_url"]

    fields = [
        {
            "name": "Source Code",
            "value": source_field(release.get("assets") or [], repo, tag),
            "inline": False,
        }
    ]
    # With nothing before it there is no honest range to link. `main...tag`
    # was what the old action defaulted to, and it answers the wrong question.
    prev = previous_tag(history, tag) if history else None
    if prev:
        fields.append(
            {
                "name": "Compare Changes",
                "value": clip(
                    f"[Compare commits](https://github.com/{repo}/compare/{prev}...{tag})",
                    FIELD_VALUE_MAX,
                ),
                "inline": False,
            }
        )
    embed["fields"] = fields[:FIELDS_MAX]
    for f in embed["fields"]:
        f["name"] = clip(f["name"], FIELD_NAME_MAX)
        f["value"] = clip(f["value"], FIELD_VALUE_MAX)

    # The notes get whatever the rest of the embed leaves, not a guessed
    # constant.
    budget = min(DESCRIPTION_MAX, EMBED_TOTAL_MAX - SAFETY - embed_size(embed))
    body = release.get("body") or "_No release notes provided._"
    embed["description"] = trim_body(body, budget, html_url)

    return {
        "username": USERNAME,
        "content": clip(content, CONTENT_MAX),
        "embeds": [embed],
    }


def post(webhook: str, payload: dict) -> int:
    """POST the payload; print the status, or Discord's error body, and return
    the exit code. The webhook URL is never printed: it is the credential.
    """
    req = urllib.request.Request(
        webhook,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            status = resp.status
    except urllib.error.HTTPError as err:
        body = err.read().decode("utf-8", "replace")
        print(f"::error::Discord answered HTTP {err.code}: {body}", file=sys.stderr)
        return 1
    except (urllib.error.URLError, OSError) as err:
        # Only the reason: URLError's repr is safe, but the request is not.
        print(f"::error::could not reach Discord: {getattr(err, 'reason', err)}", file=sys.stderr)
        return 1

    if not 200 <= status < 300:
        print(f"::error::Discord answered HTTP {status}", file=sys.stderr)
        return 1
    print(f"Discord answered HTTP {status}")
    return 0


def main(argv: list[str]) -> int:
    dry_run = "--dry-run" in argv
    args = [a for a in argv if a != "--dry-run"]
    if not args:
        print("usage: discord_release.py [--dry-run] RELEASE.json [RELEASES.json]", file=sys.stderr)
        return 2

    with open(args[0], encoding="utf-8") as fh:
        release = json.load(fh)
    if not release or not release.get("tag_name"):
        print("::error::no release object to announce", file=sys.stderr)
        return 1

    history: list[dict] = []
    if len(args) > 1:
        try:
            with open(args[1], encoding="utf-8") as fh:
                loaded = json.load(fh)
            history = loaded if isinstance(loaded, list) else []
        except (OSError, json.JSONDecodeError):
            history = []

    payload = build_payload(
        release,
        history,
        mention=os.environ.get("DISCORD_RELEASE_MENTION", ""),
        repo=os.environ.get("GITHUB_REPOSITORY", "luohoa97/cordial"),
    )

    if dry_run:
        print(json.dumps(payload, indent=2))
        print(f"embed size: {embed_size(payload['embeds'][0])} of {EMBED_TOTAL_MAX}", file=sys.stderr)
        return 0

    webhook = os.environ.get("DISCORD_WEBHOOK", "")
    if not webhook.startswith("https://"):
        print("::error::DISCORD_WEBHOOK is unset or is not an https URL", file=sys.stderr)
        return 1
    return post(webhook, payload)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
