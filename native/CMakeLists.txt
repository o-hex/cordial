cmake_minimum_required(VERSION 3.16)
project(cordial_native LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# Clang is not a preference. AOSP bionic uses C11 _Atomic inside C++ headers
# (bionic_elf_tls.h, bionic_fdsan.h) — a Clang extension GCC rejects, producing
# 144 errors on an otherwise clean tree. See docs/base-evaluation.md §2.1.
if(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  message(FATAL_ERROR
    "The native subtree requires Clang (found ${CMAKE_CXX_COMPILER_ID}). "
    "AOSP bionic does not build with GCC.")
endif()

# The AOSP bionic linker, via minecraft-linux's host wrapper. Target: `linker`.
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../third_party/mcpelauncher-linker
                 ${CMAKE_CURRENT_BINARY_DIR}/mcpelauncher-linker)

# PipeWire, for opensles.cpp's audio backend. Headers only, and only if
# present: libpipewire-0.3.so is never linked (dlopen'd at run time instead,
# see pipewire_backend.cpp), so this only decides whether that file compiles
# its real implementation or the honest "no audio" fallback it falls back to
# when pipewire-devel is absent. A tree with neither still builds.
find_package(PkgConfig QUIET)
if(PkgConfig_FOUND)
  pkg_check_modules(PIPEWIRE QUIET libpipewire-0.3)
  pkg_check_modules(SPA QUIET libspa-0.2)
endif()

# pkg-config saying "not found" is not the same as the headers being absent,
# and believing it cost this project every sample of audio it has ever tried to
# play. On the machine this was found on, `pkg-config` first on PATH is
# Homebrew's pkgconf, whose compiled-in `pc_path` is its own three Cellar
# directories and does not include `/usr/lib64/pkgconfig` — so
# `pkg_check_modules` reported libpipewire-0.3 missing on a host with
# pipewire-devel 1.6.8 installed, headers, daemon and all. The build then did
# exactly what it was told to: compiled the `#else` branch of
# pipewire_backend.cpp, and `slCreateEngine` reported
# SL_RESULT_FEATURE_UNSUPPORTED for reasons that had nothing to do with audio.
# Nothing announced this; the status message below is the only place it was
# ever visible, and it scrolls past inside a cargo build.
#
# So do not take pkg-config's word for it. Only headers are wanted here — the
# library is dlopen'd, never linked (see pipewire_backend.cpp) — and headers
# are something CMake can look for directly. `find_path` searches the platform
# include directories no matter what any one pkg-config binary believes its
# search path to be, which makes this fallback strictly more reliable than the
# thing it backs up rather than merely a second guess at it.
if(NOT PIPEWIRE_FOUND)
  find_path(PIPEWIRE_FALLBACK_INCLUDE_DIR pipewire/pipewire.h
            PATH_SUFFIXES pipewire-0.3)
  find_path(SPA_FALLBACK_INCLUDE_DIR spa/param/audio/format-utils.h
            PATH_SUFFIXES spa-0.2)
  if(PIPEWIRE_FALLBACK_INCLUDE_DIR AND SPA_FALLBACK_INCLUDE_DIR)
    set(PIPEWIRE_FOUND TRUE)
    set(PIPEWIRE_INCLUDE_DIRS ${PIPEWIRE_FALLBACK_INCLUDE_DIR} ${SPA_FALLBACK_INCLUDE_DIR})
    set(PIPEWIRE_VERSION "(version unknown: found by header search, not pkg-config)")
    message(STATUS "OpenSL ES audio: pkg-config did not find libpipewire-0.3, but its "
                    "headers are present at ${PIPEWIRE_FALLBACK_INCLUDE_DIR} and "
                    "${SPA_FALLBACK_INCLUDE_DIR}. Using those. If this is a surprise, the "
                    "usual cause is a pkg-config earlier on PATH whose pc_path excludes the "
                    "system one -- check `pkg-config --variable pc_path pkg-config`.")
  endif()
endif()

# spa-0.2 ships its own .pc and lives in its own include directory; when
# pkg-config *did* answer for libpipewire-0.3 it will not have mentioned SPA,
# and every format-negotiation header this backend includes is SPA's.
if(PIPEWIRE_FOUND AND SPA_INCLUDE_DIRS)
  list(APPEND PIPEWIRE_INCLUDE_DIRS ${SPA_INCLUDE_DIRS})
endif()

if(PIPEWIRE_FOUND)
  message(STATUS "OpenSL ES audio: pipewire-devel ${PIPEWIRE_VERSION} found; "
                  "libpipewire-0.3 will be dlopen'd at run time, not linked.")
else()
  message(STATUS "OpenSL ES audio: pipewire-devel not found at configure time "
                  "(pkg-config could not find libpipewire-0.3); slCreateEngine "
                  "will report SL_RESULT_FEATURE_UNSUPPORTED and Roblox will run "
                  "without audio. Install pipewire-devel and reconfigure to "
                  "enable it.")
endif()

# Thin extern "C" surface over the linker's C++ API, so Rust can drive it.
# Android liblog. Roblox narrates its own startup through it, so this is the
# runtime layer's most valuable diagnostic — and the source of onLogLine/onJoin.
add_library(cordial_liblog STATIC
  liblog.cpp system_paths.cpp netdb_compat.cpp opensles.cpp pipewire_backend.cpp)
if(PIPEWIRE_FOUND)
  target_include_directories(cordial_liblog PRIVATE ${PIPEWIRE_INCLUDE_DIRS})
  target_compile_definitions(cordial_liblog PRIVATE CORDIAL_HAVE_PIPEWIRE=1)

  # Checks pipewire_backend.cpp's buffer-queue fill logic (the underrun ->
  # silence rule in particular) without a live PipeWire session, a daemon, or
  # any way to produce sound — see pipewire_backend_test.cpp for why that
  # property matters here specifically. Run as part of the ordinary build
  # rather than left for someone to remember to invoke separately: a check
  # that only runs when asked for is a check that silently stops running.
  add_executable(cordial_pipewire_backend_test EXCLUDE_FROM_ALL
    pipewire_backend.cpp pipewire_backend_test.cpp)
  target_include_directories(cordial_pipewire_backend_test PRIVATE ${PIPEWIRE_INCLUDE_DIRS})
  target_compile_definitions(cordial_pipewire_backend_test PRIVATE CORDIAL_HAVE_PIPEWIRE=1)
  add_custom_command(TARGET cordial_pipewire_backend_test POST_BUILD
    COMMAND cordial_pipewire_backend_test
    COMMENT "Running pipewire_backend fill_pcm checks (no PipeWire session needed)")
  add_dependencies(cordial_liblog cordial_pipewire_backend_test)
endif()

add_library(cordial_linker_shim STATIC shim.cpp)
target_link_libraries(cordial_linker_shim PUBLIC linker)

install(TARGETS cordial_linker_shim cordial_liblog linker ARCHIVE DESTINATION lib)

# Vendored x86-64 CPU feature emulator (see third_party/libbadcpu/README.md).
# Independent of the bionic subtree; built here so there is one native build.
add_library(badcpu STATIC
  ../third_party/libbadcpu/src/badcpu.cpp
  ../third_party/libbadcpu/src/cpuid.cpp
  ../third_party/libbadcpu/src/decoder.cpp
  ../third_party/libbadcpu/src/emulator.cpp
  ../third_party/libbadcpu/src/signal_handler.cpp
)
target_include_directories(badcpu PUBLIC ../third_party/libbadcpu/include)
install(TARGETS badcpu ARCHIVE DESTINATION lib)

# JNI virtual machine. Roblox's native code calls into Java through 518 statically
# registered natives and expects a JavaVM to call back into; libjnivm supplies both
# sides without a JVM. Debug mode is what makes VM::GenerateClassDump available —
# it records the Java classes and methods Roblox actually reaches for, which is the
# only reliable way to turn the framework-API inventory into a work queue.
set(JNIVM_ENABLE_DEBUG ON CACHE BOOL "" FORCE)

# JNIVM_ENABLE_TRACE printfs *every* JNI call libjnivm handles. That is
# genuinely how most of the framework surface was discovered — `String.getBytes`
# being unresolved on the text-entry path was found in exactly this output, and
# nothing else would have shown it.
#
# It is also ruinous to leave on. The engine polls `MotionEvent.getRawX`,
# `getRawY` and `getAxisValue` per pointer per frame, so this writes several
# thousand unbuffered lines a second, forever, on the hot path. It was on
# unconditionally and it made the whole machine lag, not just Cordial.
#
# So: off by default, and one flag to get it back when diagnosing.
#
#     cmake -DCORDIAL_JNI_TRACE=ON ...
#
# `--dump-classes` remains available without it (that is JNIVM_ENABLE_DEBUG,
# above, which costs nothing per call) and answers "what did Roblox reach for"
# for most purposes. Reach for the trace when the question is "and what happened
# when it did".
option(CORDIAL_JNI_TRACE "Log every JNI call libjnivm handles (very slow)" OFF)
set(JNIVM_ENABLE_TRACE ${CORDIAL_JNI_TRACE} CACHE BOOL "" FORCE)
set(JNIVM_ENABLE_TESTS OFF CACHE BOOL "" FORCE)
set(JNIVM_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjnivm
                 ${CMAKE_CURRENT_BINARY_DIR}/libjnivm EXCLUDE_FROM_ALL)

# audio_classes.cpp needs no PipeWire headers and so is compiled
# unconditionally: pipewire_backend.h deliberately carries no PipeWire types
# (see its own header comment), so the Java audio surface builds identically
# either way and reports an honestly empty device list on a tree that has no
# backend behind it.
add_library(cordial_jni_shim STATIC jni_shim.cpp android_classes.cpp game_activity.cpp init_params.cpp accessibility.cpp cookies.cpp audio_classes.cpp deeplink.cpp clipboard.cpp)
target_link_libraries(cordial_jni_shim PUBLIC jnivm)

install(TARGETS cordial_jni_shim jnivm ARCHIVE DESTINATION lib)
