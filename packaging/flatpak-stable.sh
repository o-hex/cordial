#!/usr/bin/env bash
# Decide where the Flatpak `stable` ref points in the repository the `merge` job
# of .github/workflows/flatpak.yml is about to publish, and put it there.
#
#     flatpak-stable.sh MERGED_REPO
#
# Environment:
#   APP_ID          io.github.luohoa97.Cordial
#   PAGES_REPO_URL  the live OSTree remote, with a trailing slash
#   RELEASE_TAG     the v* tag on the commit this run built, or empty
#   NEWEST_TAG      the newest v* tag that is an ancestor of that commit, or
#                   empty; only used to warn
#   FLATPAK_GPG_HOME, FLATPAK_GPG_KEY_ID   both set, or neither
#
# **Stable is decided here, at the last moment before the tree is uploaded, and
# not by the build.** On 2026-10-15 it was decided twice too early. The run for
# the 0.28.0 commit asked "is this commit tagged?" in the first minute, before
# the tag existed. After a rerun fixed that, the run for the next commit (an
# AUR-only change) was nine minutes old and had pulled the live `stable` when it
# started, which was still 0.27.0; it deployed thirty seconds after the 0.28.0
# run and put 0.27.0 back. Pages has no incremental publish, so whichever
# deployment finishes last is the site, whatever it was built from.
#
# So this runs inside the one job that holds the `flatpak-pages` concurrency
# group from here to the deploy, reads the live channel now, and then:
#
#   tagged commit      stable becomes a new commit of this build's master,
#                      unless the live stable already has a higher version, in
#                      which case it is left alone and a warning says why;
#   untagged commit    the live stable is carried forward unchanged;
#   nothing live yet   nothing is created.
#
# "Higher" is the newest <release> in the app's own metainfo, read out of both
# commits. It needs no marker in a commit, so it works on the stable commits
# published before this script existed, and it is the version a user sees.
#
# The live ref is read with a cache-busting query string, then pulled by its
# commit id. GitHub Pages serves with `cache-control: max-age=600`, so a plain
# read a minute after the previous deploy can return the ref as it was before
# it, and carrying that forward would repeat the bug with the roles swapped.
# Objects are named by hash and cannot be stale.
set -euo pipefail

repo=${1:?usage: flatpak-stable.sh MERGED_REPO}
: "${APP_ID:?}" "${PAGES_REPO_URL:?}"
tag=${RELEASE_TAG:-}
newest=${NEWEST_TAG:-}
metainfo="/files/share/metainfo/$APP_ID.metainfo.xml"

# Highest <release version=...> in the commit's metainfo. Captured into a
# variable first: `ostree cat | sed -n 1p` style pipes close early and fail
# under pipefail.
release_version() {
  local xml
  xml=$(ostree --repo="$repo" cat "$1" "$metainfo")
  sed -n 's/.*<release version="\([^"]*\)".*/\1/p' <<<"$xml" | sort -V | tail -n 1
}

# True when $1 is strictly higher than $2.
ver_gt() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

# Prints the commit id the live site names for a ref, or nothing if the site
# has no such ref. A network failure is an error, not "no stable": treating it
# as absent would delete the release channel.
live_commit() {
  local body code
  body=$(mktemp)
  code=$(curl -sS -o "$body" -w '%{http_code}' --retry 4 --retry-delay 3 \
              --retry-all-errors "${PAGES_REPO_URL}refs/heads/$1?cb=$(date +%s%N)") || {
    echo "::error::could not read $1 from ${PAGES_REPO_URL}" >&2
    rm -f "$body"
    return 1
  }
  case "$code" in
    200) tr -d '[:space:]' <"$body" ;;
    404) ;;
    *)
      echo "::error::reading $1 from ${PAGES_REPO_URL} returned HTTP $code" >&2
      rm -f "$body"
      return 1
      ;;
  esac
  rm -f "$body"
}

# Not `refs app/$APP_ID`: given a prefix, ostree prints the names with the
# prefix stripped.
mapfile -t masters < <(ostree --repo="$repo" refs | awk -v p="app/$APP_ID/" 'index($0, p) == 1 && /\/master$/')
if [ "${#masters[@]}" -eq 0 ]; then
  echo "::error::no app/$APP_ID/*/master ref in $repo; nothing was built"
  exit 1
fi

ostree --repo="$repo" remote add --no-gpg-verify pages-live "$PAGES_REPO_URL"

for master_ref in "${masters[@]}"; do
  stable_ref="${master_ref%/master}/stable"
  new_ver=$(release_version "$master_ref")

  live_sum=$(live_commit "$stable_ref")
  live_ver=""
  if [ -n "$live_sum" ]; then
    if ! [[ "$live_sum" =~ ^[0-9a-f]{64}$ ]]; then
      echo "::error::$stable_ref on the live site is '$live_sum', not a commit id"
      exit 1
    fi
    ostree --repo="$repo" pull pages-live "$stable_ref@$live_sum"
    live_ver=$(release_version "pages-live:$stable_ref")
    echo "$stable_ref: live is $live_sum (version ${live_ver:-unknown}); this build is $new_ver"
  else
    echo "$stable_ref: not published yet; this build is $new_ver"
  fi

  move=false
  if [ -n "$tag" ]; then
    if [ "${tag#v}" != "$new_ver" ]; then
      echo "::warning::$tag is on this commit but its metainfo says $new_ver; stable will carry $new_ver"
    fi
    if [ -n "$live_ver" ] && ver_gt "$live_ver" "$new_ver"; then
      echo "::warning::$stable_ref: refusing to move stable back from $live_ver to $new_ver ($tag)"
    else
      move=true
    fi
  fi

  if [ "$move" = true ]; then
    echo "$stable_ref: $tag is on this commit; stable now points at this build"
    # A new commit, not a second name for master's. Every commit
    # flatpak-builder exports carries ostree.ref-binding naming the one ref
    # it was built for, and both `flatpak install` and `ostree pull` refuse a
    # ref whose commit does not list it ("Commit has no requested ref ...
    # stable in ref binding metadata (...master)"). An alias made with
    # `refs --create` broke every stable install from v0.17.0 and then deleted
    # stable outright on the next push.
    args=(--no-update-summary --src-ref="$master_ref")
    if [ -n "${FLATPAK_GPG_KEY_ID:-}" ]; then
      args+=(--gpg-homedir="$FLATPAK_GPG_HOME" --gpg-sign="$FLATPAK_GPG_KEY_ID")
    fi
    flatpak build-commit-from "${args[@]}" "$repo" "$stable_ref"
    final_ver=$new_ver
  elif [ -n "$live_sum" ]; then
    echo "$stable_ref: carrying the live stable ($live_ver) forward unchanged"
    ostree --repo="$repo" refs --create="$stable_ref" "pages-live:$stable_ref" --force
    final_ver=$live_ver
  else
    final_ver=""
  fi

  # The residual window: a tagged run that was displaced from the concurrency
  # queue by a newer push never publishes its stable, and the newer, untagged
  # run then carries the old one forward. Nothing here can build the missing
  # release, but it must not be silent -- silence is how 0.28.0 was lost.
  if [ -n "$newest" ] && [ -n "$final_ver" ] && ver_gt "${newest#v}" "$final_ver"; then
    echo "::warning::$stable_ref is at $final_ver but $newest is already tagged below this commit; the run that built $newest did not publish stable. Rerun it."
  fi
done

# Remote-tracking refs would be enumerated by the next step's pull-local and
# rejected as "Invalid ref name"; the carried commits are held by the local
# stable refs now.
mapfile -t remote_refs < <(ostree --repo="$repo" refs | grep '^pages-live:' || true)
for r in "${remote_refs[@]}"; do
  ostree --repo="$repo" refs --delete "$r"
done
ostree --repo="$repo" remote delete pages-live
