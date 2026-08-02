//! Stub implementations for every Android symbol Cordial does not provide yet.
//!
//! A stub records that it was called and returns zero. That makes the first
//! launch attempt produce a *prioritised* list of what to implement — which is
//! considerably more useful than a list of everything Roblox references.
//!
//! `CORDIAL_STUB_ABORT=1` aborts on the first hit instead, for bisecting a
//! specific failure.

use std::sync::Mutex;

include!(concat!(env!("OUT_DIR"), "/generated_stubs.rs"));

struct Hits {
    /// Call count per symbol index.
    counts: Vec<u32>,
    /// Symbol indices in first-hit order.
    order: Vec<usize>,
}

static HITS: Mutex<Option<Hits>> = Mutex::new(None);

fn abort_on_hit() -> bool {
    std::env::var_os("CORDIAL_STUB_ABORT").is_some()
}

fn quiet() -> bool {
    std::env::var_os("CORDIAL_STUB_QUIET").is_some()
}

/// Called by every generated stub.
pub fn hit(index: usize) -> i64 {
    let first = {
        let mut guard = HITS.lock().unwrap_or_else(|e| e.into_inner());
        let hits = guard.get_or_insert_with(|| Hits {
            counts: vec![0; SYMBOLS.len()],
            order: Vec::new(),
        });
        let first = hits.counts[index] == 0;
        hits.counts[index] += 1;
        if first {
            hits.order.push(index);
        }
        first
    };

    if first && !quiet() {
        eprintln!("[stub] {}", SYMBOLS[index].0);
    }
    if abort_on_hit() {
        eprintln!("[stub] CORDIAL_STUB_ABORT set — aborting on {}", SYMBOLS[index].0);
        report();
        std::process::abort();
    }
    if is_fatal(SYMBOLS[index].0) {
        fatal(SYMBOLS[index].0);
    }
    0
}

/// Symbols where returning `0` is not a harmless placeholder but a lie the
/// caller cannot survive.
///
/// Every stub returns `0`, which is right for most of them: a counter nobody
/// reads, a capability query answered "no". For these it is fatal, and in the
/// worst way — the caller is *told it succeeded* and proceeds.
///
/// `pthread_once` returning 0 means "your initialiser ran". It did not, so
/// whatever it was meant to set up is uninitialised and the next access is a
/// null dereference somewhere with no visible relationship to this call.
/// `pthread_getspecific` returning 0 is a NULL the caller dereferences at once.
///
/// This is precisely the failure AGENTS.md's "never make a stub lie" is about,
/// and it was found the expensive way: `cordial-run --lib-dir DIR` without
/// `--host-libc` segfaulted at exit 139 with `[stub] pthread_once` and
/// `[stub] pthread_getspecific` as the last two lines before the core dump.
/// Nobody reading a SIGSEGV would connect it to a stub returning success.
fn is_fatal(symbol: &str) -> bool {
    matches!(
        symbol,
        "pthread_once"
            | "pthread_getspecific"
            | "pthread_setspecific"
            | "pthread_key_create"
            | "pthread_key_delete"
    )
}

/// Say what happened and stop, rather than returning a lie and crashing later.
///
/// A named exit beats a SIGSEGV by the whole distance between "this symbol is
/// not implemented, here is the switch that resolves it" and a core dump three
/// frames into someone else's thread-local teardown.
fn fatal(symbol: &str) -> ! {
    eprintln!();
    eprintln!("[stub] {symbol} is not implemented, and returning a placeholder would crash.");
    eprintln!(
        "       It is thread-local storage: answering 0 tells the caller its \
         initialiser ran when it did not."
    );
    eprintln!("       Pass --host-libc to resolve libc from the host, which is what the");
    eprintln!("       --game-activity path does and why that path does not hit this.");
    report();
    // `_exit` for the same reason the loader uses it: Roblox's static
    // initialisers registered atexit handlers that expect a live Android
    // process, and running them here would replace this message with the crash
    // it exists to prevent.
    unsafe { libc_exit(1) }
}

extern "C" {
    #[link_name = "_exit"]
    fn libc_exit(status: std::ffi::c_int) -> !;
}

/// Every stub called at least once, most-called first. This is the work queue.
pub fn report() {
    let guard = HITS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(hits) = guard.as_ref() else {
        eprintln!("\n=== no stubs were called ===");
        return;
    };

    let mut called: Vec<(usize, u32)> = hits
        .order
        .iter()
        .map(|&i| (i, hits.counts[i]))
        .collect();
    called.sort_by(|a, b| b.1.cmp(&a.1));

    eprintln!(
        "\n=== stubs called: {} distinct of {} ===",
        called.len(),
        SYMBOLS.len()
    );
    for (i, count) in called {
        eprintln!("  {count:>9}  {}", SYMBOLS[i].0);
    }
}
