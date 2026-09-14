# Browser account routing

Clicking Play on Roblox's website checks the browser account and launches its
matching saved Cordial profile automatically. Routing uses Cordial's configured
secret backend: the default `auto` prefers the desktop Secret Service and falls
back to the profile's `0600` file; explicit `keyring` and `file` choices are
honoured.
The profile picker and its starting dialog stay hidden during a successful
automatic launch. Opening Cordial from its desktop icon still shows the picker.
No browser extension is required. Sign into each account in its Cordial profile
once so its identity and session are saved.

If lookup fails, no saved profile matches, or several profiles hold that account,
the launcher opens with the link waiting. Choose a profile and press Roblox. A profile
already running uses the existing busy-profile dialog; routing does not close it.
Launch errors also open the launcher so their recovery controls remain reachable.
A locked or unavailable keyring in explicit `keyring` mode retains manual
selection because there is no saved session Cordial may read.

The launcher redeems `gameinfo` once with Roblox, uses the resulting session to
request the authenticated user ID, then drops that session. Before automatic
launch it separately sends the candidate profile's saved session to the same
authenticated-user endpoint and requires the same ID. A failed or mismatched
check returns to manual profile selection and does not alter any saved profile.
The scan is read-only: it does not unlock a keyring, migrate or delete a file, or
write a keyring item. A plaintext file still takes precedence over an older
keyring value, matching what the runtime will adopt on launch. Identity snapshots
are capped at 16 KiB and cookie snapshots at 1 MiB.
The join loses its ticket before redemption starts, even when the request later
fails: a timeout cannot establish whether Roblox consumed it. Place and
private-server parameters continue through the existing join path.

Discarding or replacing the queued join invalidates its pending lookup. Changing
the selected profile during lookup also prevents automatic launch if that choice
differs when lookup completes. Set `CORDIAL_BROWSER_ACCOUNT_ROUTING=0` to retain
the original manual-selection behaviour, including ticket forwarding settings.
After the profile lock is acquired, Cordial compares the exact snapshot again
and pins that backend for the child process. A changed session or changed `auto`
result cannot silently turn the match into a launch from another store.

## Verification

Synthetic HTTP tests exercise redemption, cookie handoff, authenticated identity
and refusal. GTK tests exercise the window launch action, manual fallback,
discard and replacement. Browser account routing and the resulting engine join
have been confirmed in use. Visibility regressions test that a matching-account
launch never maps the picker, while lookup and launch failures reveal it.

The release binary was also run with an isolated data root and a synthetic
invalid ticket: it reported `browser account routing needs a profile choice`.
The full workspace tests and webview-enabled release build passed. Clippy is
blocked by existing `clippy::not_unsafe_ptr_arg_deref` errors in
`cordial-linker-sys/src/lib.rs`, outside the routing change.
