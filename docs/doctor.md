# Checking this machine

<!-- description: Run cordial --doctor to see whether this machine can run Roblox, and what to fix if it cannot. -->
<!-- icon: stethoscope -->

```bash
cordial --doctor            # add --offline to skip the update check
flatpak run io.github.luohoa97.Cordial --doctor
```

The same checks are on **Report a Problem** in the main menu, under "This
machine", and are included when you press Copy or Save to a file. Each check is
one line with a level, and anything that is not `ok` says what to do about it.

| Level | Means |
|---|---|
| `ok` | Read, and fine. |
| `info` | Worth knowing, not a fault. |
| `warn` | Roblox should still start, but something will work worse or not at all. |
| `FAIL` | Roblox will not start. This is the only level that makes `--doctor` exit 1. |

> [!TIP]
> The output shows your home directory as `~` and never names a profile, so it is
> safe to paste into an issue.

## What is checked

| Check | What it reports |
|---|---|
| Build | "Official build" when the project's own release workflows made it, otherwise "Unofficial build from" and the git remote. A hint for whoever reads a report, not a check: a fork can set the same stamp, and nothing behaves differently because of it |
| Whether it can run | `cordial-run` beside the launcher, not running as root, the Roblox archive the launcher would use and its version, and (not with `--offline`, never on the report screen) whether a newer build is on offer |
| Wayland or X11 | The game opens on X11 when `WAYLAND_DISPLAY` is unset or `CORDIAL_X11` is set. X11 is a `warn` because it is the rougher backend |
| The GPU | Vendor, device and driver, from asking Vulkan rather than the files on disk. A machine whose only Vulkan device is a CPU renderer (llvmpipe) is a `warn`: Vulkan works and the GPU is not used |
| NVIDIA | Only when Vulkan lists an NVIDIA GPU: the driver series, whether a Flatpak's GL extension matches the host driver, and whether `nvidia-drm` has `modeset` on. See below |
| Other host pieces | Vulkan driver files, sound sockets (PipeWire or PulseAudio), a keyring, GameMode when the setting is on, and whether the website's Play button opens Cordial |
| Deno | Needed by plugins that run code. Only a note when absent |
| Disk space | Where Roblox's files and the profiles live |
| Profile locks | A count: `2 of 3 profiles are open`, never which |

The GPU question is asked in a child process with a deadline, so a driver that
hangs or faults is reported and does not take the launcher with it.

### NVIDIA lines

Each says plainly when it could not read something, which is usual for `modeset`
inside a Flatpak. If Vulkan lists only the CPU renderer while the kernel sees an
NVIDIA GPU, the GPU line carries the Flatpak extension finding instead. None of
these has run against a real NVIDIA driver; [NVIDIA graphics](nvidia.md) says what
is known.

## What is not checked

Sound is "a socket is present" only. Which backend the client picks is decided
when it starts, and its own first log lines say which.
