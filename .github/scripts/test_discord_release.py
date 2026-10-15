#!/usr/bin/env python3
"""Tests for the release announcer. No network. Run: python3 .github/scripts/test_discord_release.py"""

import io
import json
import sys
import unittest
import urllib.error
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import discord_release as dr  # noqa: E402

REPO = "luohoa97/cordial"
SECRET = "https://discord.com/api/webhooks/123/SECRET-TOKEN"


def asset(name, size=0):
    url = f"https://github.com/{REPO}/releases/download/v1.2.0/{name}"
    return {"name": name, "browser_download_url": url + "x" * size}


def release(**kw):
    base = {
        "tag_name": "v1.2.0",
        "name": "Cordial 1.2.0",
        "html_url": f"https://github.com/{REPO}/releases/tag/v1.2.0",
        "published_at": "2026-10-15T09:26:39Z",
        "body": "Short notes.",
        "author": {
            "login": "github-actions[bot]",
            "html_url": "https://github.com/apps/github-actions",
            "avatar_url": "https://avatars.githubusercontent.com/in/15368",
        },
        "assets": [
            asset("cordial-1.2.0.deb"),
            asset("cordial-1.2.0.deb.cosign.bundle"),
            asset("cordial-1.2.0.rpm"),
        ],
    }
    base.update(kw)
    return base


HISTORY = [
    {"tag_name": "v1.3.0-draft", "draft": True},
    {"tag_name": "v1.2.0"},
    {"tag_name": "v1.1.0"},
]


def build(rel=None, history=HISTORY, **kw):
    return dr.build_payload(rel or release(), history, repo=REPO, **kw)


class Payload(unittest.TestCase):
    def test_shape(self):
        p = build()
        self.assertEqual(p["username"], "Cordial")
        self.assertEqual(p["content"], "New Release: `v1.2.0` in `luohoa97/cordial`")
        (e,) = p["embeds"]
        self.assertEqual(e["title"], "Cordial 1.2.0")
        self.assertEqual(e["url"], release()["html_url"])
        self.assertEqual(e["color"], 0x5865F2)
        self.assertEqual(e["description"], "Short notes.")
        self.assertEqual(e["author"]["name"], "github-actions[bot]")
        self.assertEqual(e["footer"]["text"], "Cordial loads Roblox natively on Linux")
        self.assertEqual(e["timestamp"], "2026-10-15T09:26:39Z")
        self.assertEqual([f["name"] for f in e["fields"]], ["Source Code", "Compare Changes"])
        json.dumps(p)

    def test_mention_leads_the_content_when_set(self):
        self.assertTrue(build(mention="<@&42>")["content"].startswith("<@&42> New Release:"))
        self.assertTrue(build(mention="")["content"].startswith("New Release:"))

    def test_signatures_are_dropped_and_archives_kept(self):
        value = build()["embeds"][0]["fields"][0]["value"]
        self.assertIn("cordial-1.2.0.deb)", value)
        self.assertIn("cordial-1.2.0.rpm)", value)
        self.assertNotIn("cosign", value)
        self.assertTrue(value.endswith("[TAR](https://github.com/luohoa97/cordial/tarball/v1.2.0)"))
        self.assertIn("/zipball/v1.2.0", value)

    def test_compare_uses_the_previous_tag_not_main(self):
        value = build()["embeds"][0]["fields"][1]["value"]
        self.assertIn("/compare/v1.1.0...v1.2.0", value)
        self.assertNotIn("main", value)

    def test_no_compare_field_without_a_previous_release(self):
        fields = build(history=[{"tag_name": "v1.2.0"}])["embeds"][0]["fields"]
        self.assertEqual([f["name"] for f in fields], ["Source Code"])
        fields = build(history=None)["embeds"][0]["fields"]
        self.assertEqual([f["name"] for f in fields], ["Source Code"])

    def test_missing_notes_title_and_author_have_defaults(self):
        rel = release(body=None, name="", author=None, html_url=None, published_at=None)
        now = datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
        e = build(rel, now=now)["embeds"][0]
        self.assertEqual(e["description"], "_No release notes provided._")
        self.assertEqual(e["title"], "v1.2.0")
        self.assertNotIn("author", e)
        self.assertEqual(e["url"], f"https://github.com/{REPO}/releases/tag/v1.2.0")
        self.assertEqual(e["timestamp"], "2026-01-02T03:04:05Z")


class Limits(unittest.TestCase):
    def check(self, p):
        self.assertLessEqual(len(p["content"]), dr.CONTENT_MAX)
        (e,) = p["embeds"]
        self.assertLessEqual(len(e["title"]), dr.TITLE_MAX)
        self.assertLessEqual(len(e["description"]), dr.DESCRIPTION_MAX)
        self.assertLessEqual(len(e["fields"]), dr.FIELDS_MAX)
        self.assertLessEqual(len(e["footer"]["text"]), dr.FOOTER_MAX)
        self.assertLessEqual(len(e.get("author", {}).get("name", "")), dr.AUTHOR_MAX)
        for f in e["fields"]:
            self.assertLessEqual(len(f["name"]), dr.FIELD_NAME_MAX)
            self.assertLessEqual(len(f["value"]), dr.FIELD_VALUE_MAX)
        self.assertLessEqual(dr.embed_size(e), dr.EMBED_TOTAL_MAX)

    def test_every_release_length_we_have_seen_fits(self):
        for n in (500, 5064, 5376, 6593, 9657, 100000):
            body = "\n\n".join(f"Paragraph {i} of the notes, quite wordy indeed." for i in range(n // 40))
            p = build(release(body=body))
            self.check(p)
            if len(body) > dr.DESCRIPTION_MAX:
                self.assertIn("Read the full release notes", p["embeds"][0]["description"])

    def test_short_notes_are_untouched(self):
        self.assertEqual(build(release(body="x" * 4000))["embeds"][0]["description"], "x" * 4000)

    def test_a_cut_code_fence_is_closed(self):
        body = "intro\n\n```\n" + "line\n" * 2000 + "```\n"
        d = build(release(body=body))["embeds"][0]["description"]
        self.assertEqual(d.split("\n\n**[Read")[0].count("```") % 2, 0)

    def test_cut_prefers_a_line_boundary(self):
        body = "\n".join(f"line number {i:04d}" for i in range(1000))
        d = build(release(body=body))["embeds"][0]["description"]
        cut = d.split("\n\n**[Read")[0]
        self.assertTrue(cut.split("\n")[-1].startswith("line number "))
        self.assertEqual(len(cut.split("\n")[-1]), len("line number 0000"))

    def test_asset_field_is_trimmed_to_the_cap_keeping_zip_and_tar(self):
        assets = [asset(f"pkg-{i}.tar.zst", 40) for i in range(30)]
        value = build(release(assets=assets))["embeds"][0]["fields"][0]["value"]
        self.assertLessEqual(len(value), dr.FIELD_VALUE_MAX)
        self.assertIn("[ZIP]", value)
        self.assertTrue(value.startswith("[pkg-0.tar.zst]"))
        self.check(build(release(assets=assets)))

    def test_oversized_title_author_and_content_are_clipped(self):
        rel = release(name="T" * 900, author={"login": "a" * 900})
        p = build(rel, mention="<@&1>")
        self.check(p)
        self.assertTrue(p["embeds"][0]["title"].endswith("…"))

    def test_worst_case_everything_at_once(self):
        rel = release(
            name="T" * 900,
            author={"login": "a" * 900},
            body="word " * 10000,
            assets=[asset(f"p{i}", 200) for i in range(40)],
        )
        self.check(build(rel))


class History(unittest.TestCase):
    def test_previous_tag(self):
        self.assertEqual(dr.previous_tag(HISTORY, "v1.2.0"), "v1.1.0")
        self.assertIsNone(dr.previous_tag(HISTORY, "v1.1.0"))

    def test_unlisted_tag_falls_back_to_the_newest_other_release(self):
        self.assertEqual(dr.previous_tag(HISTORY, "v9.9.9"), "v1.2.0")

    def test_empty(self):
        self.assertIsNone(dr.previous_tag([], "v1.0.0"))


class Response:
    def __init__(self, status):
        self.status = status

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class Post(unittest.TestCase):
    def run_post(self, side_effect=None, status=204):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("urllib.request.urlopen", side_effect=side_effect, return_value=Response(status)) as m:
            with redirect_stdout(out), redirect_stderr(err):
                code = dr.post(SECRET, {"content": "hi"})
        return code, out.getvalue(), err.getvalue(), m

    def test_success_prints_the_status_line(self):
        code, out, err, m = self.run_post(status=204)
        self.assertEqual(code, 0)
        self.assertIn("HTTP 204", out)
        req = m.call_args.args[0]
        self.assertEqual(req.get_method(), "POST")
        self.assertEqual(json.loads(req.data), {"content": "hi"})
        self.assertEqual(req.get_header("Content-type"), "application/json")
        self.assertIn("cordial", req.get_header("User-agent"))

    def test_http_error_fails_and_prints_discords_body_but_not_the_url(self):
        body = b'{"message": "Invalid Form Body", "code": 50035}'
        exc = urllib.error.HTTPError(SECRET, 400, "Bad Request", {}, io.BytesIO(body))
        code, out, err, _ = self.run_post(side_effect=exc)
        self.assertEqual(code, 1)
        self.assertIn("HTTP 400", err)
        self.assertIn("Invalid Form Body", err)
        self.assertNotIn("SECRET-TOKEN", out + err)

    def test_unreachable_fails_without_the_url(self):
        exc = urllib.error.URLError(OSError("name resolution failed"))
        code, out, err, _ = self.run_post(side_effect=exc)
        self.assertEqual(code, 1)
        self.assertIn("name resolution failed", err)
        self.assertNotIn("SECRET-TOKEN", out + err)

    def test_a_non_2xx_that_urllib_did_not_raise_still_fails(self):
        code, _, err, _ = self.run_post(status=302)
        self.assertEqual(code, 1)
        self.assertIn("HTTP 302", err)


class Main(unittest.TestCase):
    def setUp(self):
        import tempfile

        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.rel = Path(self.tmp.name, "release.json")
        self.rel.write_text(json.dumps(release()))
        self.hist = Path(self.tmp.name, "releases.json")
        self.hist.write_text(json.dumps(HISTORY))

    def run_main(self, args, env):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict("os.environ", env, clear=True):
            with mock.patch("urllib.request.urlopen", return_value=Response(204)) as m:
                with redirect_stdout(out), redirect_stderr(err):
                    code = dr.main(args)
        return code, out.getvalue(), err.getvalue(), m

    def test_refuses_without_a_webhook(self):
        code, _, err, m = self.run_main([str(self.rel)], {})
        self.assertEqual(code, 1)
        self.assertIn("DISCORD_WEBHOOK", err)
        m.assert_not_called()

    def test_posts_with_mention_and_history_from_the_environment(self):
        env = {"DISCORD_WEBHOOK": SECRET, "DISCORD_RELEASE_MENTION": "<@&7>", "GITHUB_REPOSITORY": REPO}
        code, out, _, m = self.run_main([str(self.rel), str(self.hist)], env)
        self.assertEqual(code, 0)
        sent = json.loads(m.call_args.args[0].data)
        self.assertTrue(sent["content"].startswith("<@&7> "))
        self.assertIn("compare/v1.1.0...v1.2.0", sent["embeds"][0]["fields"][1]["value"])
        self.assertNotIn("SECRET-TOKEN", out)

    def test_dry_run_prints_the_payload_and_sends_nothing(self):
        code, out, _, m = self.run_main(["--dry-run", str(self.rel)], {})
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)["username"], "Cordial")
        m.assert_not_called()

    def test_unreadable_history_is_not_fatal(self):
        self.hist.write_text("not json")
        env = {"DISCORD_WEBHOOK": SECRET}
        code, *_ = self.run_main([str(self.rel), str(self.hist)], env)
        self.assertEqual(code, 0)

    def test_empty_release_is_an_error(self):
        self.rel.write_text("null")
        code, _, err, _ = self.run_main([str(self.rel)], {"DISCORD_WEBHOOK": SECRET})
        self.assertEqual(code, 1)
        self.assertIn("no release object", err)


if __name__ == "__main__":
    unittest.main()
