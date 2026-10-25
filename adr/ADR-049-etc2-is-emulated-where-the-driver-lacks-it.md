---
title: "ADR-049: ETC2/EAC is decoded on the CPU where the driver lacks the feature"
---
**Status:** accepted
**Supersedes:** the "nothing is translated" half of [ADR-042](/adr/ADR-042-texture-format-query-observability); its counters and the test-only mask stand
**Related:** [ADR-001](/adr/ADR-001-in-process-hooking), [ADR-042](/adr/ADR-042-texture-format-query-observability), [ADR-046](/adr/ADR-046-nvidia-is-gated-on-the-vendor-id)

## Context

ADR-042 measured the engine's per-format ETC2 queries on Intel, found them
unchanged by masking, and concluded that the premise "NVIDIA lacks ETC2, so
something is missing" did not hold. That conclusion was drawn from the wrong
signal. The mask only ever touched `vkGetPhysicalDeviceFormatProperties`. The
engine decides whether to use ETC from the `textureCompressionETC2` device
feature, which the mask never changed, so an unchanged caps line said nothing
about the feature.

A tester on an RTX 5070 supplied the missing data point. NVIDIA reports
`textureCompressionETC2` false, the engine's caps line reads `ETC1 0 ETC2 0`,
and ETC1 built-ins such as the plastic detail texture go missing. With the
emulation below the same line reads `ETC1 1 ETC2 1`. ADR-042 named that exact
observation as what would change its mind.

## Decision

On a physical device whose `textureCompressionETC2` is false, Cordial answers
as though it were true and serves the engine's ETC2 and EAC images itself:

- the feature is reported set in `vkGetPhysicalDeviceFeatures` and
  `Features2`, and cleared again in `vkCreateDevice` so the driver is never
  asked to enable what it lacks;
- format and image-format queries for the ten ETC2/EAC formats are answered
  from the matching RGBA8, R16 or RG16 format, for sampling and transfer only;
- `vkCreateImage` and `vkCreateImageView` use that substitute format, as does
  `vkGetDeviceImageMemoryRequirements` and its sparse variant, so the image
  the driver sizes and creates is the one that is used;
- `vkCmdCopyBufferToImage(2)` decodes each region on the CPU into a staging
  buffer and copies that instead, re-checking at submit that the source did not
  change after recording.

The decoder is `etc_decode.rs` and the shim is `vulkan_etc.rs`.

**The gate is the driver lacking the feature, and not the vendor id.** ADR-046
gates the other NVIDIA behaviour on `vendorID == 0x10DE` because those
workarounds are about one vendor's driver. This one is about a capability bit.
A non-NVIDIA driver without ETC2 (a software rasteriser, an older mobile-class
part) has the identical problem and should be served the same way, and an
NVIDIA driver that grows the feature should not be shimmed. The vendor id
would be right for the first reason and wrong for the second. It also means
the `CORDIAL_FORCE_GPU_VENDOR` switch does not turn this on.

**Device-level hooks are installed on a device only if its physical device is
emulating.** An Intel or AMD device resolves the driver's own function. The
instance-level lookup cannot know which device a pointer will be used on, so
it stays hooked, and each hook forwards straight through for a device that is
not emulating. With `CORDIAL_ANDROID_TRACE` set every device is hooked so that
the passthroughs are traced.

## Controls

- `CORDIAL_NO_ETC_EMULATION=1` turns it off; the engine then sees `ETC1 0
  ETC2 0` on NVIDIA again.
- `CORDIAL_FORCE_ETC_EMULATION=1` turns it on for a driver that has ETC2
  natively, so that native and decoded output can be compared on one machine
  without NVIDIA hardware (the same family as `CORDIAL_FORCE_GPU_VENDOR`). It
  is named in `cordial-run --help` and set by no packaging script. If both are
  set, `NO` wins.

## Failure behaviour

If the engine was told ETC2 works and the state to serve it is missing, Cordial
fails by name rather than handing the driver a raw ETC format. A `vkCreateImage`
with an ETC2/EAC format on a device that is emulating but has no emulation
state (the host calls staging needs could not be resolved, or a second device
replaced the first) returns `VK_ERROR_FORMAT_NOT_SUPPORTED` and counts into
`ETC_UNHANDLED`. `CORDIAL_COUNT_GL=1` prints that counter, and a non-zero value
after joining a game is the thing to report.

A `VkPhysicalDeviceFeatures2` that is not the first node of the
`vkCreateDevice` chain cannot be copied, because the nodes ahead of it are
structures this code has no reason to size. Its one feature word is cleared in
place for the duration of the call and restored when the call returns. The
alternative, passing the feature through, is a driver that lacks it refusing to
create the device and a client that never starts.

## What is deliberately not handled

- `vkCmdBlitImage*` on an emulated image, `vkCmdCopyImageToBuffer*` from one,
  copies of an emulated image to a differently formatted one, and 3D copies
  are refused or counted rather than translated. The engine was not seen to
  make them, and `ETC_UNHANDLED` says so if that changes.
- An ETC2 format inside a `VkImageFormatListCreateInfo` is forwarded
  unchanged and counted.
- `check_pending` still decodes at submit with the shared state lock held, and
  `emulate_copy` decodes at record under it. Neither has been shown to cost a
  frame; both are known and not measured.

## Not an ADR-001 matter

Nothing here reads or writes the engine's memory or code. It is the same kind
of thing ADR-042 describes: the engine calls a function pointer Cordial handed
it, and Cordial answers at that boundary. What changed is that the answer is no
longer a pass-through, which ADR-042 said it would not be until the measurement
justified it.

## Provenance of the decoder

`etc_decode.rs` implements the ETC2 and EAC block formats as written in the
Khronos Data Format Specification (the "ETC2 Compressed Texture Image Formats"
section). It adds no third-party decoder to the dependency tree. The
intensity, distance and EAC modifier tables were read against the
specification by the reviewer from memory and not diffed against the
document. The test vectors in its `VECTORS` table, one per block mode, carry an
expected output that the test describes as produced by an independent decoder;
which decoder that was and how the vectors were generated has not been recorded
in the pull request, and is for the author to add here. Until it is, "matches
the specification" rests on the tables and the vectors agreeing with each
other, and has not been checked against real hardware output. Intel with
`CORDIAL_FORCE_ETC_EMULATION=1` against native decoding is the first such
check available without NVIDIA.

## Not yet established

- That real NVIDIA output looks right: a before and after of the plastic
  detail texture on the RTX 5070, and `CORDIAL_COUNT_GL=1` after joining a
  game with `ETC_UNHANDLED` in it, are the author's to supply.
- Staging memory for a real join. Each command buffer that uploads ETC data
  holds at least one 8 MiB staging chunk until it is reset or freed; the peak
  across a join has not been measured.
- Compatibility with a driver that lacks ETC2 and is not NVIDIA's. The code
  does not depend on the vendor, and no such driver has run it.
