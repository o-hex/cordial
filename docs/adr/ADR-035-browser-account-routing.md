# ADR-035: Match browser joins to saved accounts

**Status:** Accepted, 2026-09-15

## Context

Choosing a profile for every browser Play request is redundant when the browser
account already has one saved Cordial profile. Profile names are user-chosen, so
matching a name would be unreliable. Roblox's launch ticket can establish the
browser account through its authentication and authenticated-user endpoints.

## Decision

For the configured secret backend, redeem the browser ticket, read the
authenticated user ID and select the single saved identity with that ID. The
default `auto` choice prefers Secret Service and falls back to files; explicit
`keyring` and `file` choices keep their existing semantics. Keep the picker and
starting dialog hidden for a successful automatic join.
Opening the desktop icon still presents the picker. Failed lookups, ambiguous
matches and launch failures expose manual recovery.

A saved identity alone is not enough: identity and cookies are written
separately. Validate the candidate profile's saved session with Roblox's
authenticated-user endpoint and require the same account ID. If validation
fails or either saved value changes during the lookup, fall back to manual
selection.

Read bounded snapshots through the shared secret backend: 16 KiB for identity
and 1 MiB for cookies. Scanning is read-only. It neither unlocks the default
collection nor migrates, deletes or writes stored values. When keyring mode has
a readable plaintext value waiting to be adopted, that file has precedence over
the service value just as it does in the runtime.

The redeemed session exists only for that identity lookup. It is never copied
into a saved profile: the game uses that profile's existing session. Remove the
ticket before starting redemption, including on errors, because a timeout does
not tell us whether the server consumed it. Preserve recognised desktop launch
fields; unknown field-like text within `gameinfo` is removed with the ticket
rather than forwarded as a possible credential suffix.

Unrecognised link shapes still follow the existing manual path: the original
URL can reach `--join-url` and remain visible in `/proc/<pid>/cmdline`. Ticket
removal on the parsed path does not close that pre-existing fallback exposure.

This extends ADR-012's directory-only switcher decision: the manual switcher
still selects directories, but the shell now also knows account IDs to route a
browser join. It does not collect passwords or replace an existing profile's
authentication.

It also changes the earlier default of dropping browser credentials unless
`carry_launch_ticket` was enabled. That preference continues to govern forwarding
the ticket to the engine on the manual path. Automatic matching instead sends
the ticket to Roblox's authentication endpoint and discards the resulting
session. This distinction is deliberate, but it is still credential use: file
storage is not evidence that a user previously opted into it. The accepted
default treats a browser Play request as a request to select that browser
account. `CORDIAL_BROWSER_ACCOUNT_ROUTING=0` restores the prior manual behaviour.

Carry the selected store with the match. After taking ADR-012's profile lock,
read the same store again and require exact bytes before spawning. Pass an
explicit `CORDIAL_SECRET_STORE` to the child so a fresh process cannot make a
different `auto` choice. `None`, locked, failed, malformed and oversized reads
fail closed to manual selection. A running profile still uses ADR-012's lock and
busy-profile recovery, not an IPC that changes the running engine's account or
experience.

## Evidence and limits

Browser joins for two saved accounts were verified in the custom build, with no
picker shown during successful joins. Synthetic HTTP tests cover the exchange
without real credentials; backend tests cover file and keyring precedence,
unavailable stores, bounds and non-migration; GTK tests cover routing,
visibility and recovery. A synthetic Secret Service round trip covers keyring
readback where an unlocked service is available. This does not establish joining
into an already running client, which remains unsupported.
