# Checking this machine

```bash
cordial --doctor            # add --offline to skip the update check
flatpak run io.github.luohoa97.Cordial --doctor
```

The same checks are on **Report a Problem** in the main menu, under "This
machine", and are included when you press Copy or Save to a file. Each check is
one line with a level, and anything that is not `ok` says what to do about it:

| Level | Means |
|---|---|
| `ok` | Read, and fine. |
| `info` | Worth knowing, not a fault. |
| `warn` | Roblox should still start, but something will work worse or not at all. |
| `FAIL` | Roblox will not start. This is the only level that makes `--doctor` exit 1. |

The output shows your home directory as `~` and never names a profile, so it is
safe to paste into an issue.

## What is checked

- **Which build this is.** "Official build" when the project's own release
  workflows made it, otherwise "Unofficial build from" and the git remote it was
  built from. A hint for whoever reads a report, not a check: a fork can set
  the same stamp, and nothing behaves differently because of it. It is also the
  `Build` line of the diagnostics block, and Report a Problem's issue link goes
  to the repository an unofficial build came from when that is a GitHub one.
- **Whether it can run at all**: `cordial-run` beside the launcher, not running
  as root, the Roblox archive the launcher would use and its version, and (not
  with `--offline`, and never on the report screen) whether a newer build is on
  offer.
- **Wayland or X11.** The game opens on X11 when `WAYLAND_DISPLAY` is unset or
  `CORDIAL_X11` is set, which is the loader's own rule; X11 is a warning
  because it is the rougher backend.
- **The GPU.** Vendor, device and driver come from asking Vulkan, not from
  the files on disk. A machine whose only Vulkan device is a CPU renderer
  (llvmpipe) is a warning: Vulkan works and the GPU is not being used. The
  question is asked in a child process with a deadline, so a driver that hangs
  or faults is reported and does not take the launcher with it.
- **Vulkan driver files**, sound sockets (PipeWire or PulseAudio), a keyring,
  GameMode when the setting is on, and whether the website's Play button opens
  Cordial.
- **Deno**, which plugins that run code need. Only a note when it is absent.
- **Disk space** where Roblox's files and the profiles live.
- **Profile locks**, as a count. `2 of 3 profiles are open` and never which.

## What is not checked

The NVIDIA-specific checks, `nvidia-drm` modeset and the Flatpak GL extension
against the host driver's version, are not written yet. On a machine with an
NVIDIA GPU the doctor says so in one `info` line rather than stay silent.
`doctor::nvidia_checks` in `crates/cordial-shell/src/doctor.rs` is where they
go, and [`docs/nvidia.md`](nvidia.md) is what is known so far.

Sound is "a socket is present" only. Which backend the client picks is decided
when it starts, and its own first log lines say which.
