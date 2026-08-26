# The startup freeze, photographed

First capture of a frozen client, 2026-08-26. Everything here is one run,
`/tmp/cordial-freeze/capture-cap2`, taken with
`CAPTURE=1 tools/startup-freeze-survey.sh 2 CordialTest`.

## How to reproduce it, which is the first thing that changed

**The freeze is a signed-in phenomenon.** Twenty runs on a signed-in profile
froze sixteen times; forty runs on a signed-out one froze not once, same
machine, same binary, same evening.

```text
signed in  (ready=RootSwitchNavigator)   16 / 20 frozen   80%
signed out (ready=Landing)                0 / 40 frozen    0%
```

It also did not reproduce in seven direct launches onto the host's real GNOME
session, only inside the survey's nested headless sway. That is a second
variable and it is **not** established which of the two matters, or whether
both do. Do not read this document as saying the freeze needs a headless
compositor; it says nobody has separated them yet.

## What a frozen client is doing

Two readings, and the second one is only meaningful because of the first.

```text
CPU  103%          one full core, so it is spinning and not blocked
loopers=2
  tid=498547  fds=2  polls=516          events=17  since_event=24991ms
  tid=498578  fds=1  polls=239,323,738  events=9   since_event=25067ms
```

**239 million polls in twenty-five seconds** -- about 9.6 million a second --
on a looper with one registered descriptor that has delivered nine events in
the life of the process, the last of them twenty-five seconds ago.

The backtraces name both halves:

```text
Thread 1  (LWP 498547) "Main"        -- Cordial's own pump
  epoll_wait
  looper::looper_poll_once (timeout_millis=50, ...)   looper.rs:1620
  looper::pump                                        looper.rs:1240
  cordial_run::main                                   load.rs:4063

Thread 53 (LWP 498578) "Main"        -- an engine thread
  epoll_wait
  looper::looper_poll_once (timeout_millis=0, ...)    looper.rs:1620
  0x00007effbf398bef in ??                            (inside libroblox.so)
```

So: **a Roblox thread is calling `ALooper_pollOnce` with a zero timeout in a
tight loop, waiting for something on that descriptor that never arrives**, and
burning a core doing it. Cordial's own pump is polling normally on the other
looper and seeing nothing either -- seventeen events, none for twenty-five
seconds.

`timeout_millis=0` is the engine's choice, not Cordial's. A zero timeout means
"tell me if anything is ready and return immediately", which is a poll designed
to be called from a loop that has other work. Here there is no other work: the
loop is all there is.

## Why the CPU reading is the load-bearing one

A spinning pump and a blocked one produce **identical backtraces** -- both sit
in `epoll_wait` -- and this project has got that backwards before. Without
`103%` beside the stack, thread 53 looks like a thread waiting patiently. The
looper census is the other half: `polls=239323738` against `events=9` says the
same thing in a form that survives being pasted into an issue.

## What this does not establish

- **Which descriptor.** The looper has `fds=1` and nothing here says what it
  is or which side is supposed to write to it. That is the next measurement
  and it is the one that matters: find the fd, find who signals it on a
  healthy run, and find what stops them signalling it here.
- **Whether the main thread is a cause or a casualty.** Seventeen events and
  nothing for twenty-five seconds is consistent with both.
- **Whether the nested compositor is required**, as above.
- **Whether this is one bug.** One capture is one capture. At an 80% rate the
  second and third are minutes away and should be taken before anybody builds
  a theory on this one.

## Repeating it

```bash
CAPTURE=1 TAG=cap tools/startup-freeze-survey.sh 1 CordialTest
```

Off by default: it costs a gdb attach on every frozen run, and the survey's job
is to count freezes rather than explain one.

gdb rather than lldb, deliberately. On a genuinely deadlocked client here lldb
produced one frame per thread, twice; gdb walked the same process and named
both halves. A one-frame backtrace is not an answer.
