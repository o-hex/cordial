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

## Three captures agree, and they name the descriptor

`d1` and `d2`, taken the same way, report the same shape as `cap2` -- and the
census now names what is registered rather than counting it:

```text
spinning  fds=1  [21:1:-]                    polls 244-254 million, events=9
main      fds=2  [19:-2:cb, 27:1131377252:-] polls ~500,       events=6-18
CPU       103% in all three; the spinning thread is the only one in state R
```

`21:1:-` is fd 21, ident 1, no callback. `/proc` says what it is:

```text
19 -> pipe:[12929303]      20 -> pipe:[12929303]   (write end, same process)
21 -> pipe:[12929304]      22 -> pipe:[12929304]   (write end, same process)
27 -> socket:[12922088]                            (the Wayland display)
```

So the spinning engine thread is polling **the read end of a pipe whose write
end is open in this same process**. Nothing is closed and nothing has gone
away; something simply stops writing. Ident 1 is the app-glue's main command
channel, and `APP_CMD` appears nowhere in Cordial -- these pipes belong to the
engine's own glue inside `libroblox.so`, not to anything this project wrote.

And Cordial's main thread is starved too: its Wayland display socket has
produced no event for twenty-five seconds.

## Input does not recover a frozen client. Measured, and it refutes the obvious theory

The shape above suggests a starvation cycle -- the engine waits for a command,
the command comes from work the main thread drives, the main thread waits for
events, the events come from presenting, and presenting needs the engine. If
that were the whole story, one input event should break it.

It does not. Driven through Cordial's own entry points on a live frozen client:

```text
before                  presents=2  accepted=0
after 20 pointer moves  presents=2  accepted=20
4s later                presents=2  accepted=20
```

**The moves were accepted -- the count proves they reached the client -- and
nothing moved.** So whatever the engine thread is waiting for, an input event
is not it, and a frozen client does not come back.

This does **not** contradict the original report, and the distinction matters:
that report says input *prevents* the freeze, not that it recovers one. Those
are different claims and only the second is refuted here.

## Input does not prevent it either, with the caveat that matters

The remaining half of the original report -- that input *prevents* the freeze --
measured on a signed-in profile, ten runs per arm, strictly interleaved:

```text
arm A, no input     8 / 10 frozen
arm B, nudged       9 / 10 frozen
```

No effect, and the point estimate runs the wrong way for the theory.

**Read the instrument note before believing this arm.** It has been a silent
no-op three times in this project's history. The first two started a virtual
pointer after the client had latched seat capabilities. The third wrote into a
holder's fifo and was measured on 2026-08-27 delivering exactly nothing:
`accepted=0` on the client's own counter after a full run of it. Arm B in the
earlier ten-per-arm run was that version, and measured nothing at all.

This arm drives devctl `move`, which is the one path proven to arrive -- twenty
moves into a live frozen client took `accepted` from 0 to 20 -- and one
verification run showed `accepted=13` during startup. That run froze.

**The caveat, and it is not small.** devctl's socket only exists once the client
has bound it, which is well into startup. So this arm cannot deliver anything
during the earliest phase, and if the freeze is decided before the socket
appears, it has not been tested. What can be said is narrower than "input does
not prevent it": input delivered from the moment Cordial can accept it does not
prevent it.

## What this does not establish

- **Which side stops writing.** The descriptor is now named -- the read end of
  the engine's own app-glue command pipe -- and the write end is open in the
  same process. What is not known is what would have written to it on a healthy
  run and why it does not here. That is inside `libroblox.so`, so the way at it
  is the difference between a healthy and a frozen run's command sequence, not
  a debugger on engine code.
- **Whether the main thread is a cause or a casualty.** Seventeen events and
  nothing for twenty-five seconds is consistent with both.
- **Whether the nested compositor is required**, as above.
- **Whether this is one bug.** Three captures now agree on every reading, which
  is much better than one. They were taken minutes apart on one machine, so
  they establish a consistent shape rather than a general one.

## Repeating it

```bash
CAPTURE=1 TAG=cap tools/startup-freeze-survey.sh 1 CordialTest
```

Off by default: it costs a gdb attach on every frozen run, and the survey's job
is to count freezes rather than explain one.

gdb rather than lldb, deliberately. On a genuinely deadlocked client here lldb
produced one frame per thread, twice; gdb walked the same process and named
both halves. A one-frame backtrace is not an answer.
