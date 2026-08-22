//! The audio sinks this machine has, for the Audio row in settings.
//!
//! One question, asked of the same code the client asks: `enumerate_devices()`
//! in `native/pipewire_backend.cpp`, reached through the small C ABI declared
//! at the bottom of `native/pipewire_backend.h`. Nothing here parses
//! `pw-dump`, shells out to `pactl`, or keeps a second idea of what a device
//! is. That matters more than it sounds — the whole point of storing a
//! `node.name` is that the picker and the thing that opens the stream agree
//! about which string identifies a device, and two implementations of "list
//! the sinks" is exactly how they would come to disagree.
//!
//! **Sinks only, and the filtering happens on the C side.** The microphone
//! rule at the top of `native/audio_classes.cpp` says listing a microphone is
//! not using one, but it also says nothing on an enumeration path may
//! construct a `CaptureStream` — and the cheapest way to be certain a device
//! picker for *output* never does is for the sources never to reach Rust at
//! all. `cordial_audio_sinks` drops them before allocating.
//!
//! **Why the shell links the client's native archive for this.** It is the
//! only reason it does, and it is not free: `cargo build -p cordial-shell`
//! now needs the `third_party/mcpelauncher-linker` submodule and Clang, where
//! before it needed neither. The alternative was a second registry walk
//! compiled only into the launcher, which would have cost nothing at build
//! time and would have been a second answer to the one question this module
//! exists to have exactly one answer to. Only `pipewire_backend.o` is
//! actually extracted from the archive — a static library contributes nothing
//! for symbols nobody references, so the bionic linker and libjnivm are on the
//! link line and not in the binary.

use std::ffi::CStr;

// Declared rather than called. `cordial-linker-sys`'s build script is what
// puts `libcordial_liblog.a` on this binary's link line, and without naming
// the crate here Cargo has no dependency edge to hang that on. It is not an
// accidental leftover: deleting this line produces an undefined reference to
// `cordial_audio_sinks` at link time, which is a considerably less obvious
// message than this comment.
use cordial_linker_sys as _;

/// One sink, as the settings row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    /// `node.name` — the stable routing target that goes into `shell.json`
    /// and comes back out as `CORDIAL_AUDIO_SINK`. Never shown to the user.
    pub node_name: String,
    /// `node.description` — what their own volume control calls this device.
    /// This is the only string a user should ever read.
    pub description: String,
    /// This is the session's current `default.audio.sink`.
    pub is_default: bool,
}

#[repr(C)]
struct CordialAudioSink {
    node_name: *const std::os::raw::c_char,
    description: *const std::os::raw::c_char,
    is_default: std::os::raw::c_int,
}

extern "C" {
    fn cordial_audio_sinks(out: *mut *mut CordialAudioSink) -> usize;
    fn cordial_audio_sinks_free(sinks: *mut CordialAudioSink, count: usize);
}

/// Every audio output the PipeWire session currently has.
///
/// Empty means one of three things — no PipeWire at build time, no
/// `libpipewire-0.3.so.0` at run time, or no session behind it — and the
/// caller must present all three the same way: as "no devices found", never
/// as an invented "Default" entry. A picker offering a device that cannot
/// play is worse than an honestly empty one, because only the empty one sends
/// somebody looking for the real problem.
///
/// **This opens no stream.** It walks the registry and disconnects again; see
/// the module header.
pub fn sinks() -> Vec<Sink> {
    let mut raw: *mut CordialAudioSink = std::ptr::null_mut();
    // Safety: `cordial_audio_sinks` either leaves `raw` null and returns 0, or
    // writes an array of `count` initialised entries whose two pointers are
    // NUL-terminated and owned by the array. Freed unconditionally below.
    let count = unsafe { cordial_audio_sinks(&mut raw) };
    if raw.is_null() || count == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        // Safety: `i < count`, and the C side never writes a null into either
        // pointer — a description it does not have is filled in with the node
        // name rather than left null, precisely so this loop has no branch.
        let entry = unsafe { &*raw.add(i) };
        let node_name = unsafe { CStr::from_ptr(entry.node_name) }.to_string_lossy().into_owned();
        let description =
            unsafe { CStr::from_ptr(entry.description) }.to_string_lossy().into_owned();
        out.push(Sink { node_name, description, is_default: entry.is_default != 0 });
    }
    unsafe { cordial_audio_sinks_free(raw, count) };
    out
}

/// What the row for `sink` should read.
///
/// The description alone, except for the session's own default, which is
/// marked. Both entries are then visible at once — "System default" and the
/// device it currently resolves to — which is the difference between a picker
/// that tells somebody where their sound is going and one that makes them
/// guess.
pub fn row_label(sink: &Sink) -> String {
    if sink.is_default {
        format!("{} (current system default)", sink.description)
    } else {
        sink.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_default_is_marked_so_both_entries_can_be_read_together() {
        let d = Sink {
            node_name: "alsa_output.pci-0000_00_1f.3.analog-stereo".into(),
            description: "Built-in Audio Analogue Stereo".into(),
            is_default: true,
        };
        assert_eq!(row_label(&d), "Built-in Audio Analogue Stereo (current system default)");

        let other = Sink { is_default: false, ..d };
        assert_eq!(row_label(&other), "Built-in Audio Analogue Stereo");
    }

    #[test]
    fn asking_for_the_sinks_is_safe_with_or_without_a_session() {
        // Deliberately asserts nothing about the contents. On a developer's
        // machine this returns their real devices and in a container with no
        // PipeWire it returns none, and a test that expected either would fail
        // on the other. What it does check is the part that is the same
        // everywhere and is the part that could actually be wrong: that the
        // C ABI hands back something Rust can own without leaking or reading
        // past the end, which is what running it under the ordinary test
        // harness exercises.
        for sink in sinks() {
            assert!(!sink.node_name.is_empty(), "a sink with no node.name cannot be stored");
            assert!(!sink.description.is_empty(), "a sink with no label cannot be shown");
        }
    }
}
