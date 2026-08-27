# Signing the Flatpak remote — the procedure, for whoever holds the key

**Status: the CI side is built and waiting; nobody has generated a key.** This
document is the other half of that sentence in
[`.github/workflows/flatpak.yml`](../../.github/workflows/flatpak.yml) — "switches
on the day a maintainer adds a key" — written down precisely enough that adding
one is fifteen minutes of following steps rather than an afternoon of reading
`gpg --help` under pressure. It produces a procedure and the two secret *names*
the workflow already expects. **It contains no key material, and it should
not** — that part only a maintainer can do, on a machine they trust, and this
document cannot verify that step because it happens outside anything a coding
agent can observe.

## What the workflow already does, read rather than guessed

[verified: workflow] `.github/workflows/flatpak.yml`'s "Import the signing key"
step reads two repository secrets:

```yaml
env:
  KEY: ${{ secrets.FLATPAK_GPG_PRIVATE_KEY }}
  KEYID: ${{ secrets.FLATPAK_GPG_KEY_ID }}
```

If `FLATPAK_GPG_PRIVATE_KEY` is unset, the step prints "no
FLATPAK_GPG_PRIVATE_KEY secret; the repository will be unsigned" and the run
proceeds exactly as every run has so far. If it is set, the workflow:

1. Imports it with `gpg --homedir "$home" --batch --import` — **not**
   `--pinentry-mode loopback --passphrase`, and there is no other prompt
   anywhere in the file. **This means the imported key must need no passphrase
   to sign with**, or the import succeeds and every later `--gpg-sign` call
   hangs or fails on a runner with no terminal to answer a prompt on. This is
   the single most important constraint on the key you generate, and it is
   worth restating: **no passphrase**, not a weak one.
2. Requires `FLATPAK_GPG_KEY_ID` alongside it and fails the step outright if
   the key is present but the ID is not — so the two secrets are set together
   or not at all.
3. Signs twice with it: once in `flatpak-builder --gpg-sign="$KEYID"` when the
   OSTree commit is built, and again in
   `flatpak build-update-repo --gpg-sign="$KEYID"` when the summary and
   appstream branch are generated.
4. Exports the **public** half and appends it, base64-encoded, as a `GPGKey=`
   line to the published `packaging/cordial.flatpakrepo`. This is the only
   place the public key is ever written anywhere; the workflow never touches
   the private one after the import step, and nothing writes it to a log —
   `gpg --batch --import` does not echo key material, and no step after it
   prints `$KEY`.

None of this needs a workflow change. The moment the two secrets exist, the
next run against `main` (a push, or `workflow_dispatch`) signs. That is by
design, and `packaging/cordial.flatpakrepo`'s own comment says why: "a key
committed to the repository would be a key anyone can sign with, so the real
one only ever exists as an Actions secret."

## Generating the key

Do this on a machine you trust — a laptop, not a CI runner, and never inside a
container something else built. The point of asymmetric signing is that the
private half never has to leave a place you control until it goes straight
into GitHub's secret store, which encrypts it at rest and never displays it
again once saved.

**Make it a dedicated key, not your personal one.** Its private half is about
to become a copy-pasted GitHub Actions secret. If that secret ever leaks —
a misconfigured `pull_request_target`, a compromised runner image, a
`::add-mask::` that did not catch every code path — the cost should be a
Cordial-signing key you can revoke and replace, not an identity you use
elsewhere.

```bash
# A scratch keyring, so this does not touch your own GNUPGHOME.
export GNUPGHOME="$(mktemp -d)"
trap 'rm -rf "$GNUPGHOME"' EXIT

# --batch --passphrase '' is what makes the secret export importable by
# `gpg --batch --import` with nothing further to supply — see the constraint
# above. rsa4096 with "sign" usage only: this key is never going to encrypt or
# certify anything, so it does not need those capabilities.
#
# The expiry is a real decision, not a default to accept blindly. "2y" means
# the remote silently stops being verifiable two years from now unless
# somebody renews it in time — worth a calendar reminder the day this runs.
# "0" (never) avoids that at the cost of a key that, if it is ever compromised
# years from now with nobody watching, has no built-in end date. Either is
# defensible; pick one on purpose.
gpg --batch --passphrase '' --quick-generate-key \
    'Cordial Flatpak Remote <choose-an-address-you-monitor>' \
    rsa4096 sign 2y

# The full 40-character fingerprint, not the short 8-character ID — the short
# form is exactly what a collision attack targets, and --gpg-sign takes either.
FPR=$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr/{print $10; exit}')
echo "$FPR"

gpg --armor --export-secret-keys "$FPR" > cordial-flatpak-signing-key.private.asc
gpg --armor --export             "$FPR" > cordial-flatpak-signing-key.public.asc
```

## Storing it

1. Repository → Settings → Secrets and variables → Actions → New repository
   secret, twice:
   - `FLATPAK_GPG_PRIVATE_KEY` — the entire contents of
     `cordial-flatpak-signing-key.private.asc`, including the
     `-----BEGIN PGP PRIVATE KEY BLOCK-----` / `-----END...-----` lines.
   - `FLATPAK_GPG_KEY_ID` — `$FPR` from above, the bare 40-character
     fingerprint.
2. **GitHub secrets are write-only.** Once saved, there is no "view" button —
   only "update" and "delete" — which is the correct property for a CI secret
   and an inconvenient one for a signing key, because losing your only copy
   means the *next* key you generate cannot re-sign anything the old one
   already signed, and everyone who added the remote under the old key needs
   telling. Keep `cordial-flatpak-signing-key.private.asc` somewhere durable and
   access-controlled — a password manager's file storage, an encrypted volume,
   whatever you already trust with account recovery codes — before you delete
   the scratch keyring.
3. Delete the scratch keyring and the loose `.asc` files from wherever you ran
   the commands above once the secret is stored and the backup exists.
   `rm -rf "$GNUPGHOME"` (the `trap` above does this automatically on shell
   exit) and shred the exported files rather than just `rm` them if the disk
   is not itself encrypted.
4. Publish the fingerprint somewhere a user can check it against, out of band —
   this document, [`SECURITY.md`](../../SECURITY.md), or a pinned Discord
   message are all reasonable. A signature nobody can cross-check against a
   second source is only as trustworthy as GitHub Pages' own integrity, which
   is a smaller improvement than it looks.

## What changes for a user, and what does not

**The install commands in the README do not change.** They were written to
survive this:

```bash
flatpak remote-add --if-not-exists cordial \
    https://luohoa97.github.io/cordial/cordial.flatpakrepo
flatpak install cordial io.github.luohoa97.Cordial
```

What changes is the *content* of `cordial.flatpakrepo` at that URL — it gains a
`GPGKey=` line — and therefore what `flatpak remote-add` records locally about
that remote. **A remote added before the key existed keeps the settings it was
added with.** `flatpak update` re-fetches packages, not remote configuration, so
existing users stay on `gpg-verify=false` until they re-add it:

```bash
flatpak remote-delete cordial
flatpak remote-add --if-not-exists cordial \
    https://luohoa97.github.io/cordial/cordial.flatpakrepo
```

That is worth a release note and a pinned Discord message the day this ships,
in those words — "re-add the remote to start verifying it" is not the kind of
thing a user discovers on their own.

**Verifying it took:**

```bash
flatpak remote-info --log cordial io.github.luohoa97.Cordial   # look for a Signature line
flatpak remote-list -d | grep cordial                          # the gpg-verify column
```

Both are things to actually run once the secrets are in and a build has gone
green — this document's own rule (AGENTS.md's "verify by running") applies to
this change like any other, and nobody has run either of these yet because the
key does not exist yet.

## What this does and does not fix

Signing proves the OSTree commit was produced by whoever holds the private key.
It does not prove that person is trustworthy, and it does not replace Flathub's
review — see
[`docs/HANDOVER.md`](../HANDOVER.md#flathub-and-why-it-is-not-the-plan) for why
Flathub is a separate, currently-blocked, question. What it *does* fix is the
gap the README currently documents at length: today, anyone who can write to
the GitHub Pages site — including anyone who compromises the maintainer's
GitHub account — can serve a different package under the same name with no
warning to an installed client. A signed remote turns that into "serve a
different package signed by a key they would also have had to steal", which is
a materially smaller set of attackers.
