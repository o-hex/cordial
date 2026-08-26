# Which mirrors actually carry a Roblox build Cordial can use

Measured 2026-08-26, from this machine, on wifi. Everything here is a request
somebody can repeat; nothing is inferred from documentation.

The question was whether to add a second provider alongside APKPure, and which.
The short answer is that the one candidate with a proper documented API is
useless, the obvious one is unreachable, and the two that would work are HTML.

## The survey

| Mirror | Reachable | Newest it offers | Machine-readable | Verdict |
|---|---|---|---|---|
| **APKPure** | 200 | 2.734.917 (code 2908) | binary, no schema | in use |
| **Aptoide** | 200, documented JSON API | 2.158.48944, code 10 | JSON | **refused by our own verifier** |
| **APKMirror** | **403** | — | — | out |
| **APKCombo** | 200 | 2.735.1138 | HTML only | possible, scraping |
| **Uptodown** | 200 | 2.735 | HTML only | possible, scraping |
| **F-Droid** | 404 for this package | — | JSON | not applicable; FOSS only |

## Aptoide is the interesting failure

It is the candidate that should have won. It has a real, documented, versioned
web API -- `ws75.aptoide.com/api/7/app/getMeta?package_name=com.roblox.client`
answers 200 with clean JSON -- and VinegarHQ's own `custard` deployment tracker
watches `com.roblox.client` there, so it had a reference implementation vouching
for it.

What it serves is not a Roblox build Cordial can use, and not a recent one:

```text
vername   2.158.48944
vercode   10
size      49 217 797
store     mitapk2  (also served as store_name=apps)
```

The same answer comes back from Aptoide's own curated `apps` store, so this is
not one bad user store ranking first. `store_name=catappult` does not carry the
package at all.

Downloaded and inspected. The md5 matches what the API declared, so nothing was
tampered with in transit -- the file is what Aptoide meant to serve:

```text
209 entries
lib/armeabi/libroblox.so, lib/armeabi/libfmodex.so
abis: ['armeabi']
lib/x86_64/libroblox.so: absent
```

**armeabi only** -- 32-bit ARM, an architecture Roblox stopped shipping years
ago and one Cordial cannot execute. And `cordial_update::apk_signature` refuses
it outright:

```text
refused: the archive carries no APK signing block, so it is unsigned or
signed only with the v1 scheme, which cannot establish where it came from
```

That refusal is probably not evidence of tampering: v2 signing arrived with
Android 7 in 2016 and a genuinely ancient APK would be v1-only for honest
reasons. It is still exactly the right outcome. **Cordial cannot establish that
Roblox produced those bytes, so it will not install them**, and that judgement
was made without anybody having to notice that the version number was eleven
years stale or that the only library in it was for the wrong CPU.

This is the clearest demonstration so far of why ADR-025's condition is the
whole of the design rather than a formality. The provider with the best API is
the one serving something unusable, and the check caught it on the first
property that failed.

## APKPure is one minor version behind, and that is the real argument

Asked on the same afternoon, minutes apart:

```text
APKPure  (x-abis: x86_64)   newest 2.734.917
APKCombo (listing page)     2.735.1138, 2.734.917, 2.734.916
```

**Read that carefully before treating it as a defect.** Cordial asks APKPure
with `x-abis: x86_64`, which is deliberately narrow, and APKCombo's listing is
not filtered by architecture at all. So 2.735.1138 may well exist only for arm64
so far, in which case APKPure is not behind -- it is answering the question it
was asked. Establishing which would mean fetching 2.735.1138 and looking inside
it, and that has not been done.

What the comparison does establish is that a second provider would buy something
beyond availability if the gap turns out to be real, and that this is worth
measuring again before anybody builds one.

## If a second provider is built, APKCombo over Uptodown

Neither publishes an API. Both would mean parsing HTML, which is a different
maintenance burden from APKPure's byte-pattern reader: a reskin breaks it, and
unlike a protocol change it can break *quietly*, by matching the wrong element
and returning a plausible wrong answer.

APKCombo is the better of the two. Its old-versions listing yields clean version
strings (`2.735.1138`, `2.734.917`, `2.734.916`) in sorted order, and it exposes
per-ABI downloads, which is the filter Cordial actually needs.

Whichever is chosen, the rule from `provider::mod` applies unchanged and is what
makes trying one cheap: **it returns bytes, it never returns trust.** A scraper
that returns the wrong URL costs a wasted download. It cannot cost an install,
which is what `a_hostile_provider_cannot_get_anything_past_the_check` exists to
guarantee for exactly this case.

## Repeating this

```bash
# Aptoide
curl -sS 'https://ws75.aptoide.com/api/7/app/getMeta?package_name=com.roblox.client'

# What Cordial thinks of any APK
cargo run --release -p cordial-update --example verify-apk -- /path/to/base.apk

# What APKPure currently offers
cargo run --release -p cordial-update --example fetch_probe
```
