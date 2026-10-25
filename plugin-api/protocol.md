---
title: "Plugin API: protocol and sandbox"
description: "The wire format between Cordial and a plugin, what Deno permits, and how a plugin is started."
icon: "plug"
---
Newline-delimited JSON in both directions. It can be read by eye and with `cat`, and needs no shared memory, which would be the first step back toward the in-process access [ADR-003](/adr/ADR-003-plugin-isolation) rules out.

## Messages

A **request** is `id`, `method` and optionally `params`. `id` is yours to allocate and must be unique among your calls in flight. Omitting `params` is legal.

```json
{"id":1,"method":"flags.list","params":{}}
```

A **response** always carries `status` and the `id` it answers, in one of three shapes:

| `status` | Shape | Meaning |
|---|---|---|
| `ok` | `{"status":"ok","id":1,"result":[…]}` | The handler ran. |
| `denied` | `{"status":"denied","id":2,"capability":"flags.write"}` | The grant is missing. `capability` is the capability's wire name, not the method: `flags.set` denies as `flags.write`. |
| `error` | `{"status":"error","id":3,"message":"unknown method \"flags.nonsense\""}` | The call was allowed and failed, or the method does not exist. |

`denied` is not an `error` on purpose: "I was not allowed" and "it went wrong" send an author to different places. Check `status` before `result`, and report `capability` when you stop. *Denied, needs `flags.write`* tells the user how to fix it.

A **push** is a line Cordial sends that answers nothing. It carries `event` and `payload` and neither `id` nor `status`, which is how your dispatcher tells it from a reply:

```json
{"event":"cordial/init","payload":{"settings":{},"preferences":{}}}
{"event":"tuner/profile-changed","payload":{"slot":2}}
```

There are three kinds of push, all in `crates/cordial-runtime/src/plugin_host.rs`: the `cordial/init` handshake, another plugin's `events.publish` forwarded to its subscribers, and a core event offered to every plugin holding the capability that gates it. `cordial/` is a reserved prefix, so a line arriving with it is one Cordial published.

### The handshake

The first push is always `cordial/init`, before you have asked for anything. No capability gates it and it is not an event anyone published, so match on the name and ignore anything you do not handle.

| Field | Value |
|---|---|
| `settings` | Your saved settings document. `{}` if you hold `settings.read` and have saved nothing. `null` if you were not granted `settings.read`, the session has no profile, or the read failed (a log line says which). |
| `preferences` | The user's answers to your declared preferences, `null` under the same conditions. |

`null` means ask for the capability; `{}` means you are new here. The handshake does not list your grants. You learn what you hold by calling and reading the status.

<Warning>

Do not `await` your push handler inside the read loop. If handling a push makes calls, their replies arrive on the loop you are blocking and the plugin deadlocks on the first event. `plugins/discord-presence/main.ts` carries the comment written after exactly that.

</Warning>

### A worked exchange

A plugin granted `flags.read` and `log` but not `flags.write`. `←` is Cordial to the plugin.

```text
← {"event":"cordial/init","payload":{"settings":null,"preferences":null}}

→ {"id":1,"method":"flags.list","params":{}}
← {"status":"ok","id":1,"result":[
     {"key":"DFFlagRbxTransportUseRtcioRna","value":"false","source":"user"},
     {"key":"FIntTaskSchedulerAutoThreadLimit","value":"8","source":"plugin:tuner"}]}

→ {"id":2,"method":"log.write","params":{"message":"2 flag override(s) in effect"}}
← {"status":"ok","id":2,"result":null}

→ {"id":3,"method":"flags.set","params":{"values":{"FFlagAnything":"true"}}}
← {"status":"denied","id":3,"capability":"flags.write"}
```

Both handshake fields are `null` because `settings.read` was not granted. The last call is refused before its parameters are read: authorisation happens ahead of dispatch.

`plugins/flag-inspector/` is the shipped example of this shape. It asks for `flags.read` and deliberately not `flags.write`, so the denial is visible in a real run. `crates/cordial-plugins/tests/flag_inspector.rs` drives it against a real broker and skips itself when `deno` is not installed, so a green run is not proof it executed.

### Two mistakes to design against

**A dispatcher that does not separate a push from a reply discards every event.** A read loop that passes every line to `pending.get(msg.id)` does `pending.get(undefined)` for a push, which never matches and never throws. The event is dropped in silence. Branch on `msg.id === undefined` first, as the plugin above does and as `crates/cordial-plugins/tests/fixtures/settings.ts` and `events_subscriber.ts` do. `fixtures/roundtrip.ts` reads one line per call and is a request/response test, not a dispatcher.

**`console.log` ends the session.** See the warning on [getting started](/plugin-api/getting-started).

Two smaller shapes to check when a call quietly does nothing:

- `settings.get` takes no parameters at all: Cordial knows which process is on the pipe.
- `flags.set` wants `{"values": {…}}`. Hand it `{key, value}` and you get `flags.set needs a values object`, which is an `error`, so a plugin checking only for denials reads it as success.

## The sandbox

A plugin gets **no** Deno permissions. Cordial runs `deno run --no-prompt --quiet <entry>` with no `--allow-*` flag of any kind, and a test fails if one ever appears. There is no file, network, environment or subprocess access. The only thing a plugin can reach is the pipe.

`--no-prompt` matters: without it Deno would ask for a permission on first use and nothing would answer. With it, touching the filesystem throws immediately and you can catch and report it.

| Layer | When it applies |
|---|---|
| Deno with no permissions | Always. |
| `bwrap` | Host installs with `bwrap` present. `--unshare-all --die-with-parent --new-session`, read-only `/usr`, `/lib`, `/lib64`, `/bin` and `/etc/ssl`, a private empty `/tmp`, `HOME` and `DENO_DIR` inside that tmpfs, and your entry module only (not the plugin directory) bound read-only at `/plugin/entry.ts`. No network namespace to reach anything through. |
| Broker | Always. Every call is checked against the profile's grants. |

Cordial prints which layers you actually got, at spawn, every time:

```text
[plugin] hello: bwrap + Deno permissions + broker
[plugin] hello: Deno permissions + broker (no OS sandbox available on this install)
```

A Flatpak install deliberately gets no OS layer. Reaching `flatpak-spawn --sandbox` needs `--talk-name=org.freedesktop.Flatpak`, which also grants `--host`, arbitrary command execution outside the sandbox ([ADR-018](/adr/ADR-018-plugin-sub-sandboxing)). Without `bwrap` a plugin still has zero Deno permissions and still reaches nothing except through the broker.

<Note>

Do not rely on the OS layer hiding your home. The sandbox binds `deno` and the prefix holding its libraries read-only: the parent of `Cellar` for Homebrew, otherwise the parent of the `bin` directory the binary sits in. For a distribution package that is `/usr`. For `~/.deno/bin/deno` it binds `~/.deno`, and for `~/.local/bin/deno` it binds `~/.local`, which contains `~/.local/share/cordial/profiles/`. Nothing caps the bind to outside `$HOME`. The Deno permission layer is what stops a plugin opening any of it.

</Note>

`deno` must be on `PATH` and Cordial packages none. A missing interpreter shows as `plugin hello: could not start (…)` and the client carries on without you.

## How a plugin starts

The client process (`cordial-run`) starts plugins, not the shell, and not until the engine is up, so a misbehaving plugin cannot interfere with bring-up. For each directory under the plugin root, in sorted order, the first check that stops it prints:

| Check | Log line when it stops here |
|---|---|
| Enabled? `plugin-enabled.json` in the profile | `plugin hello: disabled in Settings, not started` |
| Has code? | `plugin hello: data only, nothing to start` |
| Granted anything? | `plugin hello: no capabilities granted, not started`. Requested but ungranted capabilities are named: `plugin hello: not granted flags.write`. |
| Spawn | The sandbox line, the `cordial/init` handshake, then `plugin hello: started`. |

After all plugins are considered the client prints `N plugin(s) running`, but only if at least one did. An absent line is not evidence that discovery did not run.

- **Enablement.** Absence from `plugin-enabled.json` means enabled, with two exceptions. First-party plugins may ship switched off (currently `fps-flex`). And the key `*` is a master switch: `{"*": false}` disables every plugin whatever its own row says. If a plugin will not start and has no row, look for that key first.
- **Roots.** Discovery and spawning read both plugin roots, system first. All four first-party plugins (`flag-inspector`, `discord-presence`, `fps-flex`, `hide-gui`) start that way. For your own plugin the user root is the easier place to iterate.
- **Ids.** Unique across the whole root. If two directories claim one, the first in sorted order wins and the second is reported and skipped, because grants, event namespaces and settings directories are all keyed by id.
- **Threads.** Each running plugin gets a thread of its own, blocking on its own stdout.
- **Grants file is the authority**, not your manifest. Settings only offers switches for capabilities you requested ([capabilities](/plugin-api/capabilities#granting-and-revoking)).
- **No restart needed.** A running client polls the profile's plugin roots, `plugin-enabled.json` and `plugin-grants.json`, and starts, stops or restarts exactly the plugin that changed ([ADR-038](/adr/ADR-038-plugin-hot-swap)). A restart after an update that changes your manifest's `capabilities` applies the intersection of what the profile granted and what the new manifest requests.
