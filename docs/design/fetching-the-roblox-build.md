# Fetching the Roblox build

**Status:** the source, the transport and the verification are implemented -- see `crates/cordial-update/src/provider/` and `apk_signature.rs`, and ADR-025. Section 5's promotion, canary and rollback are **not**: there is no previous-good record and no launch check before a build is adopted. This document was written as a specification and is now half history and half plan; the half that is plan is section 5.
clean-room exercise: it describes the observable behaviour of mocktail's update
module — endpoints, headers, byte layouts, orderings, refusals — so that
somebody who has never seen that source can build the same thing, and build it
better. mocktail is Apache-2.0 and Cordial credits it in `NOTICE`, as it already
does for the web view. **No code, no pseudocode, and no identifier of theirs
appears below.** What is stated verbatim is protocol: URLs, header names and
values, block IDs, magic numbers, byte offsets and limits. Those are facts about
APKPure's service, about Android's APK Signature Scheme, and about Roblox's
build, and getting one of them wrong produces a failure that looks like a design
problem rather than a typo.

**The decision this implements is [ADR-025](../adr/ADR-025-fetching-from-a-third-party-mirror.md)**,
which extends [ADR-015](../adr/ADR-015-fetching-the-roblox-build.md). ADR-025
says the thing worth repeating at the top of an implementation document: *the
check is the feature and the download is the convenience it buys*. Section 3
below is therefore the longest section, and it is the one to get exactly right.
Everything else can be re-tried, re-measured, or replaced.

**Where this sits beside what already exists.** `docs/design/updating-roblox.md`
is the shipped design for the header-bar button, the settings, the metered-
connection rule and the changelog, and none of that changes. `crates/cordial-update`
already has the download-with-a-byte-cap, hash-while-streaming, publish-by-rename
machinery, the engine-version reader, and the cache stamp. What is missing is a
source for the file and a proof that the file is Roblox's. This document
specifies both. An unwired `apk_signature.rs` exists in the working tree with the
v2/v3 shape already written from Google's published specification; section 3
should be read against it rather than instead of it.

## The shape of the whole thing

One run, in order. Every step can refuse, and a refusal from any step leaves the
previously working build in place and untouched.

1. Take an exclusive lock on the data root, so two updaters cannot run at once.
2. Read configuration. Refuse a source selector that is not recognised.
3. Read the catalogue of builds known to work, and pick the newest of them.
4. Inspect and verify what is currently installed.
5. Ask the provider what the newest published version is.
6. Decide what the candidate is: what is already installed, something already
   staged locally, or something to download.
7. Download the candidate's archives.
8. Verify signatures, identity and contents; extract; hash.
9. Stage the result into an immutable, content-named directory.
10. Run one or more real launches of the client against it.
11. Promote it, recording the previous activation as previous-good.
12. On any failure after step 5, keep what was installed and say why.

Steps 1–3 cost nothing. Step 5 costs one small HTTPS request. Step 7 costs
several hundred megabytes. The settings in `updating-roblox.md` govern step 7
and nothing else, which is the same split that document already argues for.

---

## 1. The metadata request

### The endpoint

```text
GET https://api.pureapk.com/m/v3/cms/app_version?hl=en-US&package_name=com.roblox.client
```

Method `GET`. The two query parameters are the whole query string: `hl=en-US`
selects the response language, and `package_name` is the Android package. Roblox's
package is `com.roblox.client` and it is a constant, not a setting.

### The headers

Four request headers beyond whatever the HTTP client sends by default. They are
what makes the endpoint answer at all; without them it does not return the
version list.

| Header | Value | What it means |
|---|---|---|
| `x-cv` | `3172501` | The APKPure client's own version code. The service uses it to decide which response shape to serve. It is a frozen constant here, and it is the field most likely to age out. |
| `x-sv` | `29` | The Android SDK level the caller is claiming. 29 is Android 10. It bounds which builds the service considers installable. |
| `x-gp` | `1` | A flag the client sends; treat it as an opaque constant that must be present. |
| `x-abis` | `x86_64` | The comma-separated ABI filter. This is the one header whose value varies at runtime. |

Set a truthful `User-Agent` of your own naming Cordial and its version. Do not
impersonate the APKPure client; the four headers above are what the service
keys on, and a borrowed user agent adds nothing but a lie.

### The ABI retry

`x-abis: x86_64` asks for the index filtered to the architecture Cordial can
actually run. That filtered index is **not reliably complete for older
versions**: APKPure sometimes omits an XAPK bundle from the filtered list even
though that bundle does contain the x86-64 split.

So there are two request patterns, and they differ:

- **Asking what the newest version is** sends `x-abis: x86_64` and never
  retries. If that answer has no version in it, the check has failed.
- **Downloading a specific version** sends `x-abis: x86_64` first, scans the
  response for a record whose version name is exactly the one requested, and if
  there is none, re-requests once with the broad filter
  `x-abis: arm64-v8a,armeabi-v7a,armeabi,x86,x86_64` and scans that instead. A
  second miss is a failure.

Widening the filter is safe because nothing downstream trusts the index about
architecture: the archive is opened and the presence of `lib/x86_64/libroblox.so`
is checked directly. The retry buys availability, not correctness.

### What comes back, and how to read it

The response is a length-delimited binary encoding — protocol-buffer-shaped,
with no published schema, no `.proto` file, and no promise of stability. It is
read by pattern-matching over bytes. Three protobuf field tags do all the work,
and knowing what they are makes the rules below legible rather than magical:

| Byte | Protobuf meaning |
|---|---|
| `0x2a` | field 5, wire type 2 (length-delimited) — the version code, as ASCII decimal digits |
| `0x32` | field 6, wire type 2 — the version name, as ASCII |
| `0x3a` | field 7, wire type 2 — whatever follows the version name; its tag byte is the ASCII colon that anchors the scan |
| `0x4a` | field 9, wire type 2 — a download URL |

Lengths are single-byte varints throughout, which is why every length check
below is against a value under 128.

**Rule 1 — find the version markers.** A version marker is a byte run that:
begins with an ASCII digit; is not immediately preceded by a byte in the set
`[A-Za-z0-9.+_-]`; continues while bytes are in that set; contains at least one
`.`; and is immediately followed by the byte `0x3a`. Collect every such run, in
file order, with the offset at which it starts and the offset of its trailing
`0x3a`.

**Rule 2 — the newest version is the first marker.** The response lists versions
newest-first. This is an observed property of the service, not a documented one;
see the weaknesses section.

**Rule 3 — recover the version code by scanning backwards.** From the first
marker's start offset, scan backwards at most 128 bytes looking for a byte
`0x2a`. For a candidate position *p*, accept it only if all of the following
hold, and otherwise keep scanning backwards:

- the byte at *p+1* is a length *L* with `1 <= L <= 20`;
- the *L* bytes from *p+2* are all ASCII digits;
- the byte at *p+2+L* is `0x32`;
- the byte at *p+3+L* equals the byte length of the version name;
- *p+4+L* is exactly the marker's start offset — that is, the name field's value
  begins where the marker begins, with nothing between.

The *L* digits, read as decimal, are the version code. Refuse a code of zero and
refuse an accumulator that would overflow. If no position satisfies all of this,
the response has a version name and no version code, and that is a failure with
its own message: the shape changed.

**Rule 4 — a record is the span between markers.** For download URLs, the record
belonging to marker *i* runs from the byte after its trailing `0x3a` to the start
offset of marker *i+1*, or to the end of the buffer for the last marker.

**Rule 5 — find download URLs inside a record.** Scan the record for the ASCII
sequence `APKJ` or `XAPKJ`. The URL begins **two bytes after the `J`** — the `J`
is the `0x4a` tag and the two bytes are a two-byte varint length, because these
URLs exceed 127 bytes. Require the URL to begin with `https://`. It continues
while bytes are alphanumeric or one of `-@:%._+~#=?&/()`. An `XAPK` prefix means
the artefact is an XAPK bundle — itself a ZIP containing several APKs; a bare
`APK` prefix means a single APK.

**Rule 6 — one URL per record, deduplicated, capped at four.** Take the first
download found in each record whose version name matches the one being fetched;
discard a URL already collected from an earlier record. If more than **4**
distinct URLs survive, refuse the whole thing rather than trying them all. If
none survive, the message is that the provider does not offer that exact version
— which is a different failure from the provider being unreachable, and the user
needs to be told which.

Every URL is checked against the download host allow-list (section 2) at the
moment it is parsed out, not later.

---

## 2. Host and transport safety

The URLs in the response come from an untrusted party. They are treated as
untrusted input all the way through.

### Allowed hosts

| Purpose | Allowed | Matching rule |
|---|---|---|
| Metadata | `api.pureapk.com` | exact |
| Downloads | `pureapk.com`, `apkpure.com`, `winudf.com` | the host, or any subdomain of it |

Subdomain matching means the candidate host must either equal the allowed name
or end with `.` followed by it. Compare case-insensitively and strip any trailing
root dot from the host first, so `API.PUREAPK.COM.` and `api.pureapk.com` are the
same host and `evilapkpure.com` is not a subdomain of `apkpure.com`.

`winudf.com` is on the download list because that is where APKPure's CDN
actually serves the bytes from. It is a third name to trust, and it is the
reason the pinned-certificate check exists.

### What a URL must be

Every URL — the initial one and every redirect target — must satisfy all of:

- scheme is exactly `https` (compared lower-cased);
- host is on the relevant allow-list above;
- if a port is present it is `443`; a URL naming any other port is refused
  rather than connected to;
- there is no userinfo component; a URL carrying credentials is refused.

Use a real URL parser for this, not string matching. `https://api.pureapk.com@evil.example/`
has host `evil.example`, and a check that looks for the allowed name as a
substring passes it.

### Redirects

**Automatic redirect following is turned off and redirects are followed by
hand.** This is not fussiness: a client that follows redirects for you validates
the first URL and then goes wherever it is sent. The loop is:

1. Issue the request with redirect-following disabled.
2. If the status is `301`, `302`, `303`, `307` or `308`, read the redirect
   target, re-validate it in full against the same rules and the same
   allow-list, and repeat. A redirect with no target is a failure.
3. At most **5** redirects, so at most 6 requests. Exceeding that is a failure
   with its own message.
4. Any other non-2xx status is a failure whose message names the host and the
   status code. "The host that refused" is the single most useful thing in that
   message, because a provider outage is the most common first-run failure and
   the user needs to know it was not their machine.

Both the protocol allow-list and the redirect protocol allow-list are set to
`https` only, so a redirect to `http://` is refused by the transport as well as
by the check.

### Byte ceilings and timeouts

| Transfer | Ceiling | Connect timeout | Transfer timeout |
|---|---|---|---|
| Metadata | 4 MiB (4 194 304 bytes) | 10 s | 60 s |
| Archive | 1 GiB (1 073 741 824 bytes) | 10 s | 15 min (900 s) |

**The ceiling is applied to bytes that arrive, not to `Content-Length`.** A
declared length is a claim by the same party supplying the bytes. Checking
`Content-Length` first is still worth doing as a cheap early refusal, but it must
never be the only check; the streaming counter is what enforces the limit.
Exceeding the ceiling aborts the transfer and produces a distinct message —
"exceeds its size limit", not a generic transport error — because a truncated
transfer and an oversized one want telling apart.

Signals are disabled on the HTTP client so a timeout cannot land as a signal in
a process that has other threads.

### How a download is written

- To a temporary name inside the destination directory, opened with `O_CREAT`,
  `O_TRUNC`, `O_CLOEXEC`, `O_NOFOLLOW`, mode `0600`. `O_NOFOLLOW` matters: the
  destination directory is under the user's cache and a symlink planted there
  must not redirect the write.
- SHA-256 is computed over the bytes as they stream past, never by re-reading
  the file afterwards.
- On completion the file is `fsync`ed, then renamed to its final name. Nothing
  downstream ever sees a partial file under the name it looks for. This is the
  same rule `crates/cordial-update/src/download.rs` already follows.
- Zero bytes written with a 2xx status is a failure ("download is empty").
- Any failure removes the temporary file.
- Progress is reported every 64 MiB.

### Candidate handling

The download directory must exist and be **empty** before anything is written to
it; a provider refuses to write into a directory another attempt already
populated. Up to four candidate archives are downloaded, named in order. **If any
one of them fails, the entire directory is removed** and the whole download is a
failure — there is no partial candidate set.

Immediately after each archive lands, its first four bytes are checked against
the ZIP local-header magics: `P`, `K`, then `0x03`/`0x05`/`0x07`, then
`0x04`/`0x06`/`0x08`. An HTML error page served with a 200 is caught here rather
than three steps later inside a ZIP reader.

---

## 3. Verification, in full

This is the load-bearing part. Everything above is convenience; this is the
reason an untrusted source is acceptable at all.

### Where the signing block is

An APK is a ZIP. The APK Signing Block sits **immediately before the ZIP central
directory** and after the last local file entry. Neither the central directory
nor the End of Central Directory record is at a fixed offset, because either can
be followed by a comment, so both are located rather than assumed.

**Find the EOCD.** Its signature is the four bytes `0x06054b50`, little-endian.
Scan backwards from `fileSize - 22`, stopping no earlier than
`fileSize - 65557` (22 bytes of fixed EOCD plus the 65 535-byte maximum comment).
Accept a candidate position *e* only if the 16-bit comment length at *e+20*
satisfies `e + 22 + commentLength == fileSize`. Without that check a file whose
data happens to contain the signature is mistaken for the record.

**Find the central directory.** Its offset is the 32-bit little-endian value at
*e+16*.

**Find the block.** The 16 bytes immediately before the central directory offset
must be the ASCII magic:

```text
APK Sig Block 42
```

— sixteen bytes, no NUL terminator. The 64-bit little-endian value at
`centralDirectoryOffset - 24` is the block's trailing size field. The block's
total size is that value plus 8, and the block therefore begins at
`centralDirectoryOffset - (size + 8)`. **The 64-bit value at that beginning must
equal the trailing size.** The block declares its own length at both ends and
they must agree; that is the check that stops a crafted footer from pointing the
parser at arbitrary earlier bytes.

Refuse if the central directory offset is below 32, or above the EOCD offset, or
if the declared size is below 24 or larger than the space available before the
central directory.

**Walk the pairs.** From `blockStart + 8` to `centralDirectoryOffset - 24`, the
block is a sequence of ID-value pairs: a 64-bit little-endian size, then a 32-bit
little-endian ID, then `size - 4` bytes of value. Require at least 12 bytes
remaining for each pair, a size of at least 4, and a size that fits in what
remains. **The walk must land exactly on the end**; a cursor that overshoots or
stops short means the block is malformed and the archive is refused.

### The block IDs

| ID | Scheme |
|---|---|
| `0x7109871a` | APK Signature Scheme v2 |
| `0xf05368c0` | APK Signature Scheme v3 |
| `0x1b93ad61` | APK Signature Scheme v3.1 |

Other IDs are present in real APKs — padding, dependency metadata, source-stamp
blocks — and are ignored. **A second pair carrying an ID already seen is a
refusal**, not a last-one-wins: a duplicate is how an attacker would try to have
the verifier check one block and the platform honour another.

**If none of the three IDs is present, the archive is refused.** That is the
v1-only refusal, and it is stated as its own message. A v1 (JAR) signature covers
individual entries rather than the file, so an archive can gain, lose or reorder
content and still verify. An archive offering nothing better than v1 has
provenance that cannot be established this way, and accepting it makes the
certificate pin decorative.

### Which scheme is checked

mocktail prefers v2 when it is present, and falls back to v3.1 before v3 when it
is not. Its stated reason is that Roblox targets Android versions below 28 and
therefore must carry v2. **Do not copy this preference; see the weaknesses
section.** Prefer the highest scheme present, and read section 3's note on the
stripping-protection attribute before deciding.

### What a signer record contains

Each scheme block is: one length-prefixed sequence, which contains a sequence of
length-prefixed signer records. All lengths are 32-bit little-endian, and every
nested sequence must be consumed exactly — trailing bytes are a refusal.

A signer record contains, in order:

1. length-prefixed **signed data**;
2. *v3 and v3.1 only:* 32-bit **minimum SDK**, 32-bit **maximum SDK**, with
   minimum no greater than maximum;
3. length-prefixed sequence of **signatures**;
4. length-prefixed **public key**, DER `SubjectPublicKeyInfo`;
5. nothing else.

The signed data contains, in order:

1. length-prefixed sequence of **content digests**;
2. length-prefixed sequence of **certificates**, each a DER X.509;
3. *v3 and v3.1 only:* 32-bit **minimum SDK**, 32-bit **maximum SDK**, which
   **must equal** the pair carried outside the signed data — that is the whole
   point of carrying them twice;
4. length-prefixed **additional attributes**;
5. for v2, current AOSP tooling appends exactly one further length-prefixed
   field of length zero. Accept precisely that and nothing else. Arbitrary
   trailing data is a refusal.

Both the digest sequence and the signature sequence are sequences of
length-prefixed records, each record being a 32-bit algorithm ID followed by a
length-prefixed byte string and nothing more. **The two sequences must have the
same length and the same algorithm IDs in the same order.** A signer offering a
signature for one algorithm and a digest for another is refused before anything
is computed.

Cap the signer count at **16**.

### Signature algorithm IDs

| ID | Algorithm |
|---|---|
| `0x0101` | RSASSA-PSS, SHA-256 digest, SHA-256 MGF1, salt length 32 |
| `0x0102` | RSASSA-PSS, SHA-512 digest, SHA-512 MGF1, salt length 64 |
| `0x0103` | RSASSA-PKCS1-v1_5, SHA-256 |
| `0x0104` | RSASSA-PKCS1-v1_5, SHA-512 |
| `0x0201` | ECDSA, SHA-256 |
| `0x0202` | ECDSA, SHA-512 |
| `0x0301` | DSA, SHA-256 |

For the PSS forms the salt length equals the digest length and the mask
generation function uses the same digest. Getting either wrong produces a
verification failure that looks like a bad archive.

Walk the signature records in order and take the **first** whose algorithm ID is
recognised and for which a digest record with the same ID exists. If none is,
the archive is refused — an unrecognised algorithm is not a reason to skip
verification.

### What is verified, and both halves are required

**Half one: the signature over the signed data.** Parse the public key from the
signer record as DER `SubjectPublicKeyInfo`, requiring the parse to consume the
whole field. Verify the selected signature over the **exact bytes of the signed
data field** with that key and that algorithm.

**Half two: the chunked content digest.** Recompute it over the archive and
compare it, in constant time, with the digest carried in the signed data under
the same algorithm ID.

Either half alone is a decoration. Half one proves the signed data was signed by
whoever holds the key; it says nothing about the file. Half two proves the file
matches the digest; the digest is inside the signed data, which whoever supplied
the archive also supplied. Only together do they say that the holder of the key
attested to these bytes.

### The chunked content digest

**Three regions are covered**, in this order:

1. from offset 0 to the start of the signing block;
2. from the central directory offset to the EOCD offset;
3. a **copy** of the bytes from the EOCD offset to the end of the file, in which
   the 32-bit field at offset 16 of that copy — the "offset of central
   directory" — is **overwritten with the offset of the start of the signing
   block**, as a 32-bit little-endian value.

The substitution in region 3 is the whole reason a signature survives the block
being inserted: at signing time the central directory sits where the block later
begins. Forget it and the digest is wrong for every valid archive, which reads
as the archive being bad rather than the verifier being wrong. That failure mode
is worth a test of its own.

The signing block itself is deliberately not covered. It cannot be: it contains
the digest.

**Chunking.** Each region is split into chunks of exactly **1 MiB
(1 048 576 bytes)**, with the final chunk of each region short. Regions do not
share chunks — a short final chunk of region 1 is not topped up from region 2.

**Chunk digest** = `H(0xa5 || uint32le(chunkByteLength) || chunkBytes)`.

**Top-level digest** = `H(0x5a || uint32le(totalChunkCount) || allChunkDigestsConcatenated)`,
with the chunk digests concatenated in region order and, within a region, in
offset order.

The two distinct prefix bytes exist so that a chunk digest and a whole-file
digest cannot collide by construction. They are one byte each and they are not
optional.

Refuse if the signing block start exceeds `0xffffffff` — the scheme cannot
express it — or if the chunk count is zero or exceeds `0xffffffff`.

### From certificates to a fingerprint

Within the signed data's certificate sequence, each entry is a length-prefixed
DER X.509. Require every entry to parse completely, with the DER parser
consuming exactly the declared length; a certificate with trailing bytes is a
refusal.

**The first certificate is the signer's leaf.** Its public key must be equal to
the public key carried in the signer record. This is the link between "the key
that made this signature" and "the certificate whose fingerprint is pinned", and
without it the pin checks a certificate nobody signed anything with. Compare the
keys structurally, not by comparing DER bytes, so an equivalent re-encoding is
not a spurious mismatch.

**The fingerprint is SHA-256 over the leaf certificate's DER encoding**, rendered
as 64 lower-case hex characters. Only the leaf's fingerprint is collected;
intermediate certificates are parsed and validated for well-formedness but
contribute no fingerprint. An empty certificate sequence is a refusal.

The result of verifying one archive is the sorted, deduplicated set of leaf
fingerprints of its signers.

### What is pinned, and where the pins live

The pinned set is a small, readable file — not a constant buried in a source
file. It carries a schema version, the package name it applies to
(`com.roblox.client`), and a list of lower-case 64-character hex digests. Each
digest is normalised to lower case and rejected unless it is exactly 64
characters drawn from `0-9a-f`. An empty list is a refusal, not "trust
everything".

Roblox's two current signing certificate fingerprints:

```text
2bebd189e8d3106401347056c93d045b61e20e22d0c3cbed85474aeb00a3d12a
44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477
```

There are two because Roblox has rotated, and both appear on current builds.
ADR-025 requires these to live somewhere a person can read and audit, and to be
updatable as a one-line change with an obvious error message when the check
fails closed.

### The base archive and the split must agree

Roblox's Android build ships as a base APK plus per-architecture splits. Cordial
needs two archives: the base, which carries `assets/`, and the `config.x86_64`
split, which carries `lib/x86_64/libroblox.so`. Some mirrors serve a monolithic
APK containing both, in which case the base and the split are the same file.

**Both archives are verified independently, and the accepted certificate set is
the intersection of three sets**: the base's leaf fingerprints, the split's leaf
fingerprints, and the pinned set. If that intersection is empty the pair is
refused with a message saying exactly that — the pair has no common trusted
signing certificate.

Requiring the intersection rather than checking each against the pins separately
is the part that matters. Two archives each signed by a *different* pinned
certificate would each pass an independent check, and pairing them would install
a library from one release beside assets from another. Android's own installer
enforces the same rule for the same reason.

When the base and the split are the same file, verify it once and use the result
for both; do not verify a 900 MB archive twice to satisfy the shape of the rule.

---

## 4. What is checked about the payload after extraction

Signature verification says Roblox produced these bytes. It does not say they
are the bytes Cordial asked for, or that they contain a usable engine. Four
further checks follow, and each has its own message.

### Archive identity

The Android manifest inside each candidate is `AndroidManifest.xml`, stored as
Android binary XML rather than text. What is needed from it is the package name,
the version name, the version code, and the split name.

Parsing rules, stated as bytes:

- The file is a chunk tree. Every chunk header is a 16-bit type, a 16-bit header
  size, and a 32-bit chunk size. Require header size at least 8, chunk size at
  least header size, and chunk size within the remaining buffer.
- The outermost chunk has type `0x0003` and its size must equal the whole file.
- Type `0x0001` is the string pool. Its header is at least 28 bytes: a 32-bit
  string count at offset 8, 32-bit flags at offset 16, and a 32-bit offset to the
  string data at offset 20. Flag bit `0x00000100` means the strings are UTF-8;
  otherwise they are UTF-16LE. An array of 32-bit offsets follows the header, one
  per string, relative to the string data start. Cap the count at a million and
  refuse a second string pool.
- UTF-8 strings carry two varint-ish lengths — character count then byte count,
  each one byte if the high bit is clear and two bytes otherwise, big-endian
  within the two — followed by the bytes and a NUL that must be present. UTF-16
  strings carry one such length in 16-bit units, one or two 16-bit words. Refuse
  a surrogate rather than guessing at it.
- Type `0x0102` is a start element. Its header size must be exactly 16, and its
  chunk size at least 36. The tag's string index is the 32-bit value at offset
  20. The 16-bit values at offsets 24, 26 and 28 are the attribute array's start
  (relative to offset 16), the per-attribute stride, and the attribute count; the
  stride must be at least 20.
- Only the element whose tag is `manifest` is read. Within each attribute, the
  32-bit value at +4 is the name's string index, the value at +8 is the raw
  string index, the byte at +15 is the value's type, and the 32-bit value at +16
  is the typed data.
- A string-valued attribute takes the raw string index, or, when that is the
  no-index sentinel `0xffffffff` and the type byte is `0x03`, the typed data as a
  string index.
- The version code is read from the typed data only when the type byte is in the
  integer range `0x10`–`0x1f`.
- Attributes are matched by their name string alone: `package`, `versionName`,
  `split`, `versionCode`. Namespace resolution is not attempted, and does not
  need to be, because no other element is read.
- A manifest with no package or a zero version code is refused.

**What the identity must satisfy:** package exactly `com.roblox.client`; version
code exactly the one requested; version name exactly the one requested — with one
documented exception, that a **split** APK may legitimately carry no version name
at all, in which case a non-empty split name stands in for it. A candidate that
fails any of this is silently skipped rather than refused, because a bundle
legitimately contains splits for other architectures; if no candidate survives,
*that* is the refusal, worded as "no archive matches the requested version".

**Pairing:** the base is the first surviving candidate with an empty split name
that contains any entry under `assets/`. The split is the first surviving
candidate containing `lib/x86_64/libroblox.so` whose split name is either
`config.x86_64` or empty. Missing either is a refusal.

### Archive extraction rules

Exactly two things are taken out: `lib/x86_64/libroblox.so` from the split, and
everything under `assets/` from the base. The extraction refusals in
[ADR-014](../adr/ADR-014-plugin-registry-and-unpacking.md) apply in full, and
`crates/cordial-update/src/apk.rs` already implements them. The additional rules
worth naming:

- An entry name is unsafe if it is empty, begins with `/`, contains a NUL or a
  backslash, or has any path component that is empty, `.`, or `..`. A trailing
  empty component (a directory's trailing slash) is the only empty component
  allowed.
- An entry whose Unix mode — the high 16 bits of the external file attributes —
  says symbolic link is refused. ZIP cannot express a hard link, so there is no
  field to check and no refusal to write; say so rather than carrying a check
  that can never fire.
- Cap the declared entry count at 100 000 and refuse an archive that declares
  more, or whose declared count disagrees with what enumeration finds.
- Every output file is opened with `O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW`,
  mode `0600`.
- The bytes written must equal the entry's declared uncompressed size, and the
  entry's CRC must check out on close. A truncated extraction is refused and the
  partial file removed.
- Cap the total extracted asset bytes at 2 GiB and any single APK at 1 GiB.
- When extracting a whole prefix, the listing is taken once and then re-checked
  entry-by-entry as extraction walks the archive; a name, size, directory flag or
  symlink flag that differs between the two passes means the archive changed
  underneath and the extraction is refused.

### The ELF

`libroblox.so`, once extracted, must be:

- a 64-bit ELF (`ELFCLASS64`) for machine `EM_X86_64`;
- carrying a section named `.note.android.ident` — the marker that says this
  object was built for Android rather than for a desktop Linux, and the cheapest
  way to notice a substituted library;
- carrying a GNU build ID of exactly 20 bytes, rendered as 40 lower-case hex
  characters;
- exporting all of a fixed list of symbols, defined rather than undefined.

The required exports are Roblox's own JNI entry points, and their absence means
the engine will fail at a point with no relationship to the cause:

```text
JNI_OnLoad
Java_com_roblox_engine_jni_NativeGLInterface_nativeGameGlobalInit
Java_com_roblox_engine_jni_NativeGLInterface_nativeUpdateAdapterInit
Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2InitWithParams
Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeStartLuaAppDM
Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2StartAppWithParams
Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams
```

Read them from `.dynsym`, skipping entries with no name or an undefined section
index. Cordial's own list should be the symbols Cordial's runtime actually calls;
the list above is what mocktail requires, and it is a reasonable starting set
because it is the JNI surface any working build must publish.

If a build ID was expected — because the version being fetched is one already in
the catalogue of known-good builds — it must match exactly. A version name and
version code that match while the build ID does not means the mirror served a
different binary under the same version, and that is a refusal, not a warning.

### The assets, and the recorded hashes

The asset tree must contain at least one file, and must contain a directory the
runtime expects (in Roblox's layout, `assets/content`). An empty extraction with
a successful exit is a real failure mode and it is worth catching by name.

Four hashes are recorded, all SHA-256, all lower-case hex:

- the extracted library;
- the base archive as downloaded;
- the split archive as downloaded;
- an **asset tree hash**, computed by listing every regular file under the asset
  root relative to it, sorting those paths as strings, and folding
  `digest || "  " || relativePath || 0x00` for each in order into a running
  SHA-256. The file count is recorded beside it.

The tree walk refuses a symbolic link or any non-regular, non-directory entry
outright rather than skipping it.

### The version and build-ID catalogue

A small readable file lists the builds that have been checked by a human and are
known to work, one entry per build, each carrying at minimum a version name, a
version code, an ELF build ID, a status, a default-allowed flag, and a free-text
reason recording what was verified and when.

The loader accepts an entry only if its status is `supported`, it is marked
default-allowed, and it does not request any legacy binary-patching behaviour;
everything else is skipped. Version name must be non-empty, version code
non-zero, and build ID exactly 40 lower-case hex characters. A file that yields
no acceptable entry is itself a failure — there is no implicit "anything goes".

The **preferred** profile is the acceptable entry with the highest version code.
It is the fallback target when the provider cannot be reached and nothing is
installed.

The same record — package, version, build ID, the four hashes, the asset count,
where the archives came from and when — is written beside each installed payload
and is what the store's integrity check reads back. For Cordial the equivalent
already exists in `cordial_update::cache`'s stamp; the useful addition is
recording the four hashes and the build ID beside it, so that "is the installed
engine still the one that was verified" is answerable without a network.

---

## 5. Promotion and rollback

### What a canary is

A canary is **a real launch of the real client against the candidate payload**,
in a throwaway environment, whose log is then read for evidence that the engine
actually reached a frame. It is not a smoke test of the updater; it is the
engine running.

The isolation is the interesting part, because a canary that pollutes the user's
profile is worse than no canary:

- A fresh temporary directory is created under the cache root, containing five
  subdirectories that become the child's home, data, cache, state and config
  roots. Every XDG variable and every application-specific root variable points
  inside it. The directory is removed when the canary ends, however it ends.
- The child's environment is **built, not inherited**. Only a named allow-list
  passes through: display and session (`DISPLAY`, `WAYLAND_DISPLAY`,
  `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `XAUTHORITY`), audio
  (`PULSE_SERVER`, `PIPEWIRE_REMOTE`), locale (`LANG`, `LANGUAGE`, `TZ`, `USER`,
  `LOGNAME`), and graphics driver selection (`VK_DRIVER_FILES`,
  `VK_ICD_FILENAMES`, `MESA_LOADER_DRIVER_OVERRIDE`, `DRI_PRIME`,
  `__NV_PRIME_RENDER_OFFLOAD`, `__VK_LAYER_NV_optimus`,
  `__GLX_VENDOR_LIBRARY_NAME`). `PATH` is set to a fixed minimal value.
  A later revision adds `LD_LIBRARY_PATH` and the GTK/GIO/GSettings search
  variables, because distributions that resolve shared libraries through a
  launcher wrapper — NixOS is the named case — cannot find their GL libraries
  without them. That addition is worth knowing about in advance: the first
  version of this list is always too short, and the symptom is a canary that
  fails only on somebody else's distribution.
- Standard output and standard error are redirected to a log file created with
  `O_EXCL | O_NOFOLLOW`, mode `0600`, under a canary log directory named for the
  graphics backend, the process ID and a monotonic timestamp.
- The child is told to exit a few seconds after its first present, to ignore a
  window-close request, to skip its own update check, and to require real
  graphics rather than falling back to software.
- Timeout is 150 seconds by default, bounded to 1–600. On timeout the child gets
  `SIGTERM`, then `SIGKILL` five seconds later, and the run is a failure.

### How many run, and when

| Candidate | Canaries |
|---|---|
| A build already in the catalogue, matching by version name, version code and build ID | **1** |
| A build not in the catalogue | **2** |

Two runs for an unknown build exist because one clean run is not a result; the
same rule AGENTS.md states for every other claim in this repository. Skipping
canaries entirely is possible for a catalogued build and **refused** for an
uncatalogued one — there is no path that activates an unknown build without
evidence.

### What counts as success

All of the following, or the run failed:

- the child exited with status 0 (not signalled, not timed out);
- the log contains every required readiness marker for the selected graphics
  backend — for a Vulkan run, that the first frame was presented and that the
  windowing adapter shut down cleanly; for a GL run, that an EGL context and
  surface came up, that the context is ES 3.x, and that a first frame was
  presented;
- the log contains a **numbered** present or swap counter with a value greater
  than zero. This is the check that distinguishes a client which initialised a
  loader and a window from one that reached the real queue. A marker saying
  "initialised" is satisfied by a great deal of code that never draws.
- the log contains no fatal marker: no `[FATAL]`, no `Segmentation fault`, no
  `core dumped`;
- audio evidence is self-consistent: if buffers were submitted, buffers were
  consumed and the audio path shut down cleanly with nothing pending. A canary
  that submits audio into a void is a failing canary even though it drew frames.

### What is kept on failure

**The current payload.** A failed candidate is never promoted, and the previously
active build is not touched at any point in the attempt — promotion is the last
step and it is a single atomic rename. The candidate's downloaded and verified
payload stays staged in the store, so a retry does not re-download several
hundred megabytes.

The result message names what was kept and why the candidate was rejected. Not
"update failed": "candidate *X* was rejected: *reason*; kept *Y*".

### The marker that prevents a retry loop

A background check that runs at every launch will re-download and re-canary a
candidate that cannot work, every launch, forever. So a rejection writes a marker
file whose name is the tuple

```text
<payload id> - <runtime build id> - <graphics backend>
```

with the reason as its contents, capped at 4096 bytes. While that file exists, a
**startup preflight** — the automatic, unattended check — sees the marker and
declines to retry, keeping the current payload and saying so. An explicit,
user-initiated update is not gated by it.

Keying the marker on the runtime's own build ID is the part that makes it
correct rather than merely convenient: a rebuilt Cordial has a different build
ID, so a fix in Cordial automatically re-enables the candidate without anybody
remembering to clear anything. Keying it on the graphics backend does the same
for a user who switches from Vulkan to GL.

### When metadata is unreachable but a working payload is installed

**This is not an error.** The run succeeds, reports the installed payload, and
says that update metadata is temporarily unavailable, quoting why. Nothing is
downloaded and nothing changes.

That branch is the whole reason the provider check happens before anything
expensive. It is also the branch that `docs/design/updating-roblox.md` already
argues for in a different context: a check that cannot reach its endpoint must
say so, naming what it could not reach, rather than rounding to "you are up to
date".

If metadata is unreachable and **nothing** is installed, the run falls back to
the newest catalogued profile and downloads that by exact version. A first-run
user with a flaky network gets a known-good build rather than nothing.

Two more early exits worth copying:

- The provider's newest version equals what is installed and already approved:
  report it and stop, without downloading.
- The installed version code is **higher** than what the provider reports:
  report "installed is newer than provider metadata" and stop. Never
  automatically downgrade because a mirror's index went backwards.

### The store, promotion, and rollback

Payloads live in a content-named directory: `<version code>-<40-hex build id>`.
Its properties:

- Staging copies the prepared tree into the store and then **re-verifies it in
  place**, rehashing everything, before the directory is given its final name.
  The bytes that were checked must be the bytes that landed.
- The staged tree is made read-only — every file `0444`, every directory
  `0555` — before publication.
- A directory that already exists under the target name is compared; if it is
  byte-identical the stage is a no-op, and if it is not, the existing directory
  is **quarantined under a timestamped name rather than overwritten**. Two
  different sets of bytes claiming the same content-derived name is a fact worth
  keeping, not deleting.
- The whole store is guarded by an exclusive lock, separate from the updater's
  own lock.

Promotion writes an activation record naming the payload ID, its relative path,
the version, the build ID and the library hash. The write is atomic: temporary
file, `fsync`, rename, then `fsync` of the containing directory. Before it is
overwritten, the existing activation record is copied to a **previous-good**
record — but only when it names a different payload, so re-promoting the same
build does not erase the rollback target.

Rollback reads the previous-good record, **re-verifies the payload it names**
including its hashes, and only then writes it back as the activation record.
A rollback that trusts the record is a rollback into whatever the record now
points at.

Verifying the current payload is a separate operation from inspecting it: the
inspect path checks layout, schema and identity without rehashing several
hundred megabytes, and the verify path rehashes everything. Staging and promotion
use verify; the fast startup path uses inspect. Naming those two things
differently, and making the expensive one the default for anything that changes
state, is worth carrying over.

### The part not to carry over

mocktail also has an approval-receipt scheme for builds that are *not* in the
catalogue: it derives a per-build profile of engine internals by matching
normalised x86-64 instruction signatures against an already-approved reference
build, binds that profile plus two canary attestations into a hash-chained
receipt, and requires the receipt to re-validate before the payload will run.
It is careful work and the receipt design is genuinely interesting — immutable
`0444` artefacts, ownership and permission checks on read, a generation hash over
all the evidence, path validation that refuses anything resolving outside its own
directory.

**Cordial cannot have the thing it exists to enable.** Deriving RVAs of engine
internals in order to interpose on them is exactly what
[ADR-001](../adr/ADR-001-in-process-hooking.md) and
[ADR-003](../adr/ADR-003-plugin-isolation.md) rule out permanently. Cordial runs
the build as shipped or does not run it. So the receipt machinery has no job here
and should not be built; what transfers is the *shape* — evidence recorded
immutably, re-validated on use, and a promotion that cannot happen without it.

---

## 6. The provider chain

### The abstraction

A provider is three things:

| Operation | Answers |
|---|---|
| name | a short stable identifier, recorded in receipts and quoted in error messages |
| check-latest | the newest published version name and version code, or a reason it could not say |
| download-exact | for a given version name and an empty output directory, a set of archive paths, or a reason |

A provider that only serves pinned versions reports an error from check-latest
and stays usable through download-exact. That is a legitimate provider, not a
broken one, and the chain must not treat "cannot tell you what is newest" as
"cannot serve you anything".

### How failures are reported

The chain tries providers **in order** and the first success wins. Each failure
is accumulated as `name: reason`, joined with `; `, and the aggregate becomes the
chain's error only if every provider failed. Nothing is swallowed: a user whose
update failed sees which sources were tried and what each one said.

Each provider is given **its own subdirectory** of the output directory, named
for the provider. Providers refuse a non-empty directory, and a failed attempt
must not poison the next one's workspace.

Every archive from every provider goes through the same signature, identity, ABI
and hash verification afterwards. **The chain is an availability mechanism, not a
trust mechanism.** Adding a provider does not add a party that has to be trusted,
which is precisely what makes adding one cheap.

### What is configurable and what is not

Configurable, in the user's configuration file:

- whether updates happen automatically at all;
- a source selector, which accepts `auto` or the historical value `apk-pure`
  (both resolve to the same chain — the old name is kept so an existing
  configuration keeps working);
- a reserved flag for whether a desktop integration relaunches after an update,
  which the updater itself never acts on.

An unrecognised key under the updates section is an **error**, not a warning; a
key that has been retired produces a warning and is ignored, so a stale
configuration file degrades loudly rather than silently. The configuration file
must be a regular file no larger than 1 MiB, opened `O_NOFOLLOW`, and a missing
file means defaults rather than failure.

Not configurable, by design: the allowed hosts, the pinned certificates, the byte
ceilings, the timeouts, the redirect limit, the number of canaries, and the
catalogue of known-good builds. Each of those is a security or correctness
property, and a setting that can turn one off is a setting somebody will be
talked into turning off.

### What the chain actually contains

**One provider.** The abstraction, the ordered iteration and the aggregated error
message all exist; the chain is constructed with a single member. There is a
second provider implemented as a standalone script — it fetches a *pinned* XAPK
from a specific mirror by file ID, with the archive's SHA-256 pinned alongside it
in a manifest — but it is a first-run bootstrap, not a fallback, and it is not
wired into the chain. See the weaknesses section.

---

## 7. Every failure mode

Every row leaves the previously working build in place. "State left behind" below
describes what else changes.

| What goes wrong | How it is detected | What the user is told | State left behind |
|---|---|---|---|
| Another updater is already running | Exclusive lock on the data root refused | Cannot lock the updater | Nothing |
| Configuration names an unknown source | Selector is neither `auto` nor the legacy name | Only the known providers are supported | Nothing |
| Configuration has an unknown key | Key not in the accepted set | Names the key | Nothing |
| Configuration has a retired key | Key on the retired list | Warning that it is ignored | Run continues |
| Catalogue missing, oversized, or unparseable | File read bounded at 4 MiB, schema version checked | Names the file and the reason | Nothing |
| Catalogue has no acceptable profile | No entry is supported, default-allowed and free of legacy patches | No default-supported build is available | Nothing |
| Metadata request fails at transport | Connect or transfer error | Names the host and the transport failure | Nothing downloaded |
| Metadata answers non-2xx | Status outside 200–299 after redirects | Names the host and the status code | Nothing downloaded |
| Metadata redirects off the allow-list | Per-hop host validation | Redirected to an untrusted host | Nothing downloaded |
| Metadata exceeds 4 MiB | Streaming byte counter | Response exceeds its size limit | Nothing downloaded |
| Metadata has no version marker | No marker matches the scan rules | Contains no version identity | Nothing downloaded |
| Metadata has a name but no code | Backward scan finds no valid tag pair | Contains no version code | Nothing downloaded |
| Metadata unreachable, payload installed | Any of the above, plus a verified current payload | Metadata temporarily unavailable, quoting why; reports the installed build | **Success.** Nothing changes |
| Metadata unreachable, nothing installed | Same, with no current payload | Falls through to the newest catalogued build | Downloads the fallback |
| Requested version not offered | No marker equals the requested version, after the broad-ABI retry | Provider does not offer that exact version | Nothing downloaded |
| More than four candidate URLs | Deduplicated URL count exceeds 4 | Provider returned too many candidates | Nothing downloaded |
| Download output directory not empty | Emptiness check before writing | Output directory must exist and be empty | Nothing |
| Archive exceeds 1 GiB | Streaming byte counter | Download exceeds its size limit | Temporary file removed |
| Archive download returns non-2xx or zero bytes | Status and byte counter | Names host and status, or "download is empty" | Temporary file removed; whole candidate directory removed |
| Archive is not a ZIP | First four bytes are not a ZIP local-header magic | Response is not a ZIP archive | Whole candidate directory removed |
| No EOCD | Backward scan over the last 65 557 bytes finds no valid record | Archive has no valid ZIP end record | Downloads retained for the run, workspace removed at the end |
| No signing block | Magic before the central directory absent | Archive has no Signature Scheme v2/v3 block | As above |
| Signing block sizes disagree | Leading and trailing size fields differ | Block size headers disagree | As above |
| Signing block pairs malformed | Pair walk overshoots or stops short | Pair is truncated, or has an invalid size | As above |
| Duplicate scheme block | Same ID seen twice | Duplicate signature scheme block | As above |
| **v1-only archive** | None of the three scheme IDs present | Archive has no supported signing scheme | As above |
| Signer fields malformed | Length-prefixed parse leaves residue | Names which field and the residue length | As above |
| v3 SDK range mismatched | Outer pair differs from signed-data pair | Signed data has a mismatched SDK range | As above |
| Digest and signature algorithms disagree | Sequences differ in length or ID order | Digest and signature algorithms disagree | As above |
| No supported algorithm | No recognised ID with a matching digest | No supported matching signature and digest | As above |
| Public key malformed | DER parse incomplete | Signer public key is malformed | As above |
| **Signature does not verify** | Verification over the signed data fails | Signer signature verification failed | As above |
| **Content digest does not match** | Recomputed chunked digest differs | Content digest verification failed | As above |
| Leaf key does not match certificate | Key comparison fails | Public key does not match its certificate | As above |
| **Certificate not pinned** | Intersection of base, split and pinned sets is empty | Pair has no common trusted signing certificate | As above |
| Pin file invalid or empty | Schema, package or digest format check | Names the specific defect | As above |
| Manifest is not binary XML | Outer chunk type or size wrong | Not valid binary XML | As above |
| Manifest identity wrong | Package, version code or version name mismatch | No archive matches the requested version | As above |
| No base/split pair | Pairing rules find no base or no split | No matching base and x86_64 pair was found | As above |
| Unsafe archive entry | Path or symlink refusal | Names the offending entry | As above |
| Extraction truncated | Written bytes differ from declared size, or CRC fails | Entry is truncated, or failed its CRC check | Partial file removed |
| Extraction exceeds its cap | Running byte total | Exceeds its size limit | As above |
| Archive changed mid-extraction | Re-listing disagrees with the first listing | Archive changed during extraction | As above |
| Library is not an x86-64 Android ELF | Class, machine, or `.note.android.ident` absent | Names which | As above |
| Library missing a required export | `.dynsym` scan | Names the missing symbol | As above |
| Build ID absent or wrong length | Note not 20 bytes | Build ID is not 20 bytes | As above |
| **Build ID is not the expected one** | Compared against the catalogue entry | Downloaded build ID is not exact-supported | As above |
| Assets empty | Extracted file count is zero | Base archive contains no assets | As above |
| Prepared payload fails its own re-verify | Rehash disagrees with recorded hashes | Names the file whose hash mismatched | Prepared tree discarded |
| Staged payload identity changed | Post-copy verify disagrees | Staged payload identity changed | Staging directory removed |
| Store name collision with different bytes | Existing directory verified and compared | Nothing user-visible; recorded | Existing directory quarantined |
| Canary cannot start | Runtime binary is not an executable regular file | Cannot start the canary | Isolated roots removed |
| Canary times out | Deadline reached | Canary timed out | Child killed; isolated roots removed; **rejection marker written** |
| Canary exits non-zero | Exit status | Exited with status *N*; names the log path | Rejection marker written |
| Canary log missing a marker | Log scan | Names the missing marker | Rejection marker written |
| Canary presented no frames | No positive present or swap counter | Real queue present is missing | Rejection marker written |
| Canary log shows a crash | `[FATAL]`, `Segmentation fault`, `core dumped` | Log contains a fatal process failure | Rejection marker written |
| Canary audio inconsistent | Submitted without consumption, or unclean shutdown | Names which audio path | Rejection marker written |
| Candidate already rejected for this runtime | Marker file for (payload, runtime build ID, backend) exists | Already failed with this runtime; kept the current build | Nothing; no download attempted |
| Promotion cannot write atomically | Temp-write, fsync or rename fails | Cannot publish the payload manifest | Temporary removed; previous activation intact |
| Rollback with no previous-good | Record absent | No previous-good payload | Nothing |
| Rollback target fails verification | Rehash of the named payload | Names the failure | Activation record unchanged |
| Everything failed and nothing was installed | Candidate rejected and the catalogued fallback also failed | Both reasons, joined | Nothing installed |

---

## Where this design is weak

Stated plainly, because the implementer needs to know what not to copy as much
as what to build. Several of these are already anticipated in ADR-025's "what
this costs" section; the rest are things reading the module made visible.

**A hand-written parser for an undocumented wire format, and it will break
quietly.** The version scan accepts any dotted token followed by a colon. A
changelog string, a URL, a date rendered `1.2.3:` inside some other field — any
of those becomes a "version". The recovery is guarded well (the backward scan
requires four separate byte agreements before it accepts a version code), so a
false marker usually produces a clean failure rather than a wrong answer. But
"the first marker is the newest version" rests on an ordering the service never
promised, and the failure mode when the response shape changes is *an update
that silently stops finding new versions*, which looks exactly like Roblox not
having shipped one. Whatever is built here needs a way to notice that it has
stopped working: a stored "last successfully parsed" timestamp surfaced in the
update dialog is a cheap version of that.

**The client headers are a frozen snapshot of somebody else's client.**
`x-cv: 3172501` is APKPure's own version code. It will age, the service will one
day stop serving that generation, and the symptom will be an HTTP error or an
empty response rather than "your client is too old".

**One provider behind an abstraction built for several.** The chain iterates, the
errors aggregate, and there is exactly one member. The apparent second source is
a pinned single-file entry — a page URL, a numeric file ID, an archive size and
an archive SHA-256, all baked into a manifest — for one specific version. That is
a bootstrap for a first install, not a fallback: it can only ever produce the one
version pinned into it, and it goes stale the moment the catalogue moves on.
mocktail knows this, which is why a separate drift-checking script exists whose
entire job is to notice that the preferred supported profile has no pinned
bootstrap source. **A second source that needs a script to warn you it has gone
stale is not a second source.**

**A dependency on one mirror's uptime, with nothing behind it.** ADR-025 already
records that the mirror under consideration answered 503 for a full day. A single
mirror is a single point of failure and the chain does not currently mitigate it.

**Certificate pinning is right and the pin file is only as trustworthy as the
install.** There is no signature over the pin file, and no path to update it
without shipping a new package. When Roblox rotates, every download fails closed
— correct direction, but the recovery is a release. The message needs to make
adding the new digest a copy-paste, and there needs to be *some* audited path to
add one without waiting for a release.

**The scheme preference is backwards, and one attribute is unchecked.** v2 is
preferred over v3 and, when v2 is absent, v3.1 is preferred over v3. Two
problems. First, v3.1 exists precisely to carry a *rotated* signer that older
platforms must not see, and it is normally accompanied by a v3 block; treating a
v3.1 block as an ordinary v3 block and stopping there is not obviously right, and
v3's proof-of-rotation lineage is not evaluated at all, so a pinned old
certificate and a legitimately rotated new one cannot be related to each other.
Second, and more seriously: the v2 signed-data **additional attributes are parsed
and then ignored**. The stripping-protection attribute (`0xbeeff00d` in v2 signed
data, carrying the highest scheme version the signer also applied) is the
mechanism that stops an attacker removing the v3 block and having the verifier
fall back to v2. A verifier that checks v2 first and ignores that attribute has
no downgrade protection. **Prefer the highest scheme present, and honour the
stripping-protection attribute.** This is the one place in the module where a
change is a security fix rather than a preference.

**The canary is the right idea on the wrong axis for Cordial.** Requiring a real
windowed launch with real graphics before promotion is strong evidence, and it
fails on exactly the machines that most need a working update: headless, remote,
software-rendered, or simply a compositor having a bad afternoon. The rejection
marker keyed on runtime build ID and backend is a genuinely good design and it
does not help a user who never rebuilds Cordial. Nothing ever expires or clears a
marker, so one transient failure is remembered permanently for that combination.

**Four candidate archives are downloaded before any is inspected.** With a 1 GiB
cap each, that is up to 4 GiB of transfer to answer a question the first archive
usually answers. There is no free-space check before starting — the header for
one is included and never used — so the failure on a small disk is a write error
several hundred megabytes in.

**Downgrade handling is asymmetric.** "Installed is newer than the provider says"
is handled explicitly. "The provider reports a *lower* version because its ABI
filter changed" is the same observation with a different cause, and it is treated
as the first.

**The asset-tree hash's separator is two spaces and the path, with no length
prefix.** Sorted order and the trailing NUL make a practical ambiguity unlikely,
but a length-prefixed fold costs nothing and removes the question.

**Progress is an ad-hoc datagram protocol on an inherited file descriptor** — a
one-byte tag and a human-readable string. It works, and it bakes in the
assumption that the updater is a separate process. Cordial's updater is a module
in the same process with a `glib` timeout already collecting answers off a
channel; do not import the descriptor.

**The derived-compatibility-profile machinery is large, brittle, and forbidden
here anyway.** Disassembling the candidate and matching normalised instruction
signatures against a reference build, in order to locate internals to interpose
on, is precisely what ADR-001 and ADR-003 rule out. It is also the single largest
file in the module. Leave the whole thing alone.

---

## What a better version would do

Ranked by value, with a judgement on each of the suggestions in the brief and
some additions.

### Try the APK the user already has, before any network at all

**Strongly yes, and this is the highest-value item on the list.** Cordial already
reads Sober's downloaded APK at
`~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk`,
and AGENTS.md records that three agents in succession failed to find it. That
file is a legitimate provider in exactly the sense section 6 describes: it cannot
answer "what is newest", and it can absolutely answer "give me these bytes". Put
it first in the chain.

It costs no network, no mirror uptime, no undocumented parser, and no trust —
because it goes through the same signature check as a download. It also converts
today's awkward dependency into a feature: the README stops saying "install a
different Roblox client first to obtain a file" and starts saying "if you already
have Sober, Cordial will use its build and check it".

The one subtlety: the local APK's version is whatever Sober last fetched, which
may be older than the newest published. Report that honestly — "using the build
already on this machine, version *X*; a newer one is available" — rather than
treating a local hit as the end of the question.

### Verify a locally-supplied APK with the same check

**Yes, and it should be the only trust path.** ADR-025 already requires it and
says why better than this document can: *"it would be strange to verify the
convenient path and not the manual one, and the manual one is where the 'modded
APK' advice actually lands."* Today Cordial trusts a hand-picked file because a
person picked it. Once the verifier exists, that is a strictly worse position
than the download it was meant to be safer than.

Concretely: the APK picker on the Roblox settings page runs the full section 3
check before the path is accepted, and the picker's error message on failure says
which check failed. A file that fails is not silently rejected; the user is told
that the archive is not signed by Roblox, which is information they cannot get
any other way.

### A second provider that is genuinely independent

**Yes, but be honest about what it buys.** It buys *availability* and nothing
else — the signature check is what makes any source acceptable, so a second
mirror adds no trust and removes no risk. That is exactly why it is cheap to add
and why the bar for adding one should be low.

What it must not be is mocktail's pinned-manifest arrangement: a single URL, a
single file ID and a single hash for one version, which produces the fallback
that only works until the catalogue moves and then fails in a way somebody has to
write a script to notice. If a pinned entry is kept at all, keep it as an
explicitly-labelled *bootstrap* — "a known-good build from a known-good URL, for
a first install with nothing else available" — and make its staleness visible in
the UI rather than in a CI job.

The chain worth building, in order:

1. the local Sober APK, if present;
2. a user-supplied file, if configured;
3. mirror A;
4. mirror B;
5. the pinned bootstrap, labelled as such.

Each returns bytes; none returns trust.

### Surface *why* an update failed

**Yes, and Cordial is already most of the way there.** `cordial_update::Unreachable`
already distinguishes the three network cases — nothing answered, something
answered with a refusal, something answered with a body this code cannot read —
and the third variant exists specifically because a 200 whose shape changed must
not present as "no update available". That is the right taxonomy and it needs one
more arm:

- **Unverified**: the bytes arrived and did not prove to be Roblox's. Carry which
  check failed (no signing block / v1 only / signature / content digest /
  certificate not pinned / base and split disagree) and, for a pin mismatch, the
  observed fingerprint.

Then never collapse any of these into one message. The failure table in section 7
is the enumeration; each row should be reachable as a distinct sentence. A
provider outage, a stale parser and a tampered archive are three completely
different things for a user to do something about, and one generic "update
failed" makes all three unactionable.

Two specific messages worth writing carefully because they will be read most:

- The **host that refused** belongs in a status-code failure. A provider outage
  is the most common first-run failure, and "apkpure.com answered HTTP 503" tells
  the user it is not their machine.
- A **pin mismatch** should print the observed fingerprint beside the pinned
  ones. Either Roblox rotated, in which case that hex string is the fix, or
  something served a build Roblox did not sign, in which case it is evidence.

### Make the pinned set auditable and updatable without a code change

**Yes, with a constraint.** The pins in a readable file, as ADR-025 requires, and
loaded at runtime rather than compiled in. Add a user-level override that can
only **add** to the set, never remove, and only after an explicit confirmation
that says in plain words what is being agreed to. Log every load of a pin that
came from the override rather than the shipped file, so a support bundle shows it.

The constraint: an additive-only override is still a mechanism by which somebody
can be talked into trusting the wrong certificate. That is acceptable only
because the alternative — failing closed until a release ships — is a worse
outcome the day Roblox rotates. Make the confirmation dialog carry the fingerprint
being added and the sentence "this tells Cordial to accept builds signed by a key
it did not previously trust".

### Additions

**Honour stripping protection and prefer the highest scheme.** Repeated here
because it is the one item on this page that is a security fix rather than an
improvement.

**Check free space before downloading, and stop at the first archive that
verifies.** Instead of downloading up to four candidates and then choosing, take
them one at a time and stop when a base/split pair verifies. In the common case
that is one archive rather than four.

**Do not gate promotion on a graphical canary.** Cordial's cheaper equivalents
already exist and are better instrumented: the engine's version read straight out
of the ELF (`cordial_update::engine`), and the present counter available through
`cordial_screenshot`/`cordial_info` on the development control surface
([ADR-019](../adr/ADR-019-development-control-surface.md)). What is worth keeping
from the canary design is the *structure*: an isolated data root, an environment
built rather than inherited, a hard timeout with escalation, and evidence read
out of a log rather than inferred from an exit code. What is worth dropping is
requiring a real window before an update is allowed at all.

**Make rollback a rename.** Keep the previous extracted engine directory rather
than overwriting in place, and switch by renaming. That turns "the new build does
not start" from a re-download into an instant revert, and it is far cheaper than
canaries for the same outcome. It pairs naturally with the cache stamp
`cordial_update::cache` already writes.

**Adopt the quarantine rule.** When a content-named directory already exists with
different bytes, move it aside under a timestamped name instead of overwriting.
It costs a rename and it preserves the evidence for the one bug where it matters.

**Skip the binary XML parser if the pairing can be decided another way.** The
manifest is parsed for four fields, and three of them are only needed to decide
which archive is the base and which is the split. If Cordial identifies the split
by the presence of `lib/x86_64/libroblox.so` and the base by the presence of
`assets/`, and takes the version from the ELF's own version string (which
`cordial_update::engine` already reads), then a few hundred lines of hand-written
chunk parsing over an attacker-supplied file disappear. Weigh that against the
fact that the manifest is the only place the *declared* version code lives; if
the catalogue keys on build ID rather than version code, it is not needed at all.

**Record what was verified, and re-check it cheaply on startup.** The distinction
between a full rehash and a layout-and-identity inspection is worth keeping.
Startup should do the cheap one; anything that changes state should do the
expensive one.

**Say out loud, in the README, that Cordial downloads Roblox from a mirror.**
ADR-025 asks for this in those words. It is a sentence about what this project
is, and it should appear rather than be discovered.
