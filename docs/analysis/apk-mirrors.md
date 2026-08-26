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
| **APKCombo** | 200 | 2.735.1138 (ARM only) | HTML only | possible, scraping |
| **Uptodown** | 200 | 2.735 (unchecked) | HTML only | possible, scraping |
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

## APKPure is not behind. Settled, and the narrow filter is why

Asked on the same afternoon:

```text
APKPure  (x-abis: x86_64)   newest 2.734.917
APKCombo (listing page)     2.735.1138, 2.734.917, 2.734.916
```

That looked like APKPure trailing by a version, and it is not. Asked again with
the broad filter, `x-abis: arm64-v8a,armeabi-v7a,armeabi,x86,x86_64`, APKPure
returns 2.735.1138 as its newest -- so it **has** the build and deliberately
withheld it from the narrow answer.

Settled by reading each artefact's ZIP central directory over HTTP range
requests, which costs about 600 kB rather than the 276 MB the two files weigh.
APKPure offers 2.735.1138 as two XAPK bundles, and here is every entry in each:

```text
variant 1 (142 443 839 bytes)      variant 2 (133 928 223 bytes)
  99 207 485  com.roblox.client.apk    85 203 261  com.roblox.client.apk
  43 226 350  config.armeabi_v7a.apk   48 714 968  config.arm64_v8a.apk
       6 477  icon.png                      6 477  icon.png
       2 965  manifest.json                 2 959  manifest.json
```

**No `config.x86_64.apk` in either.** 2.735.1138 is published for 32-bit ARM and
64-bit ARM and not for x86-64, so there is nothing in it Cordial could run, and
2.734.917 genuinely is the newest x86-64 build APKPure has.

This retires the freshness argument for a second provider. It also turns a
design decision into a measured one. `mirror.rs` sends the narrow filter when
asking what the newest version is and **never retries broad**, on the reasoning
that widening it would let an ARM-only release become "the newest version" and
send every step afterwards chasing a build that cannot start. That was written
as a hypothetical. It is now a thing that happened, on the current release, on
the day it was checked.

The broad retry stays where it belongs: on *downloading a named version*, where
the archive is opened and checked for `lib/x86_64/libroblox.so` directly, so
widening buys availability and cannot buy a wrong answer.

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
# Is a given version built for x86-64? Reads the central directory only.
curl -sSL -r 0-1 -D - -o /dev/null "$XAPK_URL"        # total size from Content-Range
curl -sSL -r $((TOTAL-200000))-$((TOTAL-1)) "$XAPK_URL" -o tail.bin

# Aptoide
curl -sS 'https://ws75.aptoide.com/api/7/app/getMeta?package_name=com.roblox.client'

# What Cordial thinks of any APK
cargo run --release -p cordial-update --example verify-apk -- /path/to/base.apk

# What APKPure currently offers
cargo run --release -p cordial-update --example fetch_probe
```
