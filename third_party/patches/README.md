# Patches to the vendored submodules

**These are changes Cordial has made to `third_party/` submodules that cannot be
committed as submodule pointers**, and this directory exists so they are not
lost.

`third_party/mcpelauncher-linker` and its nested `bionic` point at
`minecraft-linux` repositories nobody here can push to. Committing into a
submodule locally moves Cordial's recorded pointer to a commit that exists on
one machine and nowhere else, so `git submodule update` fails for every clone —
which is a worse outcome than a dirty tree, and is why the tree has been dirty
rather than the pointer wrong.

So the diff lives here instead. It is the source of truth for these changes;
the working tree is a convenience.

## What is in them

`bionic-cordial-tracing.patch` — tracing hooks in the guest-visible `libdl.so`
and the linker, all of them behind environment variables and inert when unset:

| Variable | What it prints |
|---|---|
| `CORDIAL_TRACE_DLSYM` | every `dlopen` and `dlsym` the engine makes through the guest's own `libdl.so`, with the resolved address |
| `CORDIAL_TRACE_DLADDR` | every `dladdr`, with the file name and base it answered |

They exist because the alternative is reading a stripped binary, which is the
mistake `AGENTS.md` opens by warning about. The `dlsym` trace is unfiltered on
purpose: deciding which symbols count before running anything is precisely the
guessing it replaces.

## Applying them

```bash
git -C third_party/mcpelauncher-linker/bionic apply ../../patches/bionic-cordial-tracing.patch
```

## Getting rid of them

Two ways, both better than this directory. Upstream them, or fork the
submodules under an account this project controls and point `.gitmodules` at
the fork. Until one of those happens, a fresh clone builds a Cordial without
these traces and nothing warns you.
