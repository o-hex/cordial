# ADR-035: Match browser joins to saved accounts

Status: proposed

## Context

Choosing a profile for every browser Play request is redundant when the browser
account already has one saved Cordial profile. Profile names are user-chosen, so
matching a name would be unreliable. Roblox's launch ticket can establish the
browser account through its authentication and authenticated-user endpoints.

## Decision

For explicit `CORDIAL_SECRET_STORE=file` launches, redeem the browser ticket,
read the authenticated user ID and select the single saved identity with that
ID. Keep the picker and starting dialog hidden for a successful automatic join.
Opening the desktop icon still presents the picker. Failed lookups, ambiguous
matches and launch failures expose manual recovery.

A saved identity alone is not enough: identity and cookies are written
separately. Validate the candidate profile's saved session with Roblox's
authenticated-user endpoint and require the same account ID. If validation
fails or either file changes during the lookup, fall back to manual selection.

The redeemed session exists only for that identity lookup. It is never copied
into a saved profile: the game uses that profile's existing session. Remove the
ticket before starting redemption, including on errors, because a timeout does
not tell us whether the server consumed it. Preserve recognised desktop launch
fields; unknown field-like text within `gameinfo` is removed with the ticket
rather than forwarded as a possible credential suffix.

This extends ADR-012's directory-only switcher decision: the manual switcher
still selects directories, but the shell now also knows account IDs to route a
browser join. It does not collect passwords or replace an existing profile's
authentication.

It also changes the earlier default of dropping browser credentials unless
`carry_launch_ticket` was enabled. That preference continues to govern forwarding
the ticket to the engine on the manual path. Automatic matching instead sends
the ticket to Roblox's authentication endpoint and discards the resulting
session. This distinction is deliberate, but it is still credential use: file
storage is not evidence that a user previously opted into it. The proposed
default treats a browser Play request as a request to select that browser
account. `CORDIAL_BROWSER_ACCOUNT_ROUTING=0` restores the prior manual behaviour.

Only file-backed identities are supported. Other secret backends keep manual
selection; changing their storage or migrating sessions is not part of this
decision. A running profile still uses ADR-012's lock and busy-profile recovery,
not an IPC that changes the running engine's account or experience.

## Evidence and limits

Browser joins for two saved accounts were verified in the custom build, with no
picker shown during successful joins. Synthetic HTTP tests cover the exchange
without real credentials; GTK tests cover routing, visibility and recovery.
This does not establish keyring-backed routing or joining into an already
running client. Those remain unsupported.
