/* Guest code for tests/code.rs: code the guest writes, runs, rewrites and
 * runs again, as a JIT does. The host functions arrive as pointers, as in
 * m1.c. Each generated function is `movz w0, #v; ret`, so what it returns
 * says which version of it ran. */

typedef unsigned long size_t;

struct code_api {
    void *(*mmap)(void *, size_t, int, int, int, long);
    int (*munmap)(void *, size_t);
    int (*mprotect)(void *, size_t, int);
};

#define PROT_READ 1
#define PROT_WRITE 2
#define PROT_EXEC 4
#define MAP_PRIVATE 0x02
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define PAGE 4096

typedef unsigned (*gen_fn)(void);

static void emit(void *at, unsigned v) {
    volatile unsigned *p = at;
    p[0] = 0x52800000u | ((v & 0xffff) << 5); /* movz w0, #v */
    p[1] = 0xd65f03c0u;                       /* ret */
}

/* mmap RW, write v1, RX, call; RW, write v2, RX, call. Returns the two
 * results as (first << 16) | second, or a negative step number. */
long guest_code_rewrite(const struct code_api *api, unsigned v1, unsigned v2) {
    void *p = api->mmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == (void *)-1)
        return -1;
    emit(p, v1);
    if (api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0)
        return -2;
    unsigned a = ((gen_fn)p)();
    if (api->mprotect(p, PAGE, PROT_READ | PROT_WRITE) != 0)
        return -3;
    emit(p, v2);
    if (api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0)
        return -4;
    unsigned b = ((gen_fn)p)();
    api->munmap(p, PAGE);
    return ((long)a << 16) | b;
}

/* mmap RW, write v1, RX, call; munmap; mmap RW at the same address with
 * MAP_FIXED, write v2, RX, call. */
long guest_code_remap(const struct code_api *api, unsigned v1, unsigned v2) {
    void *p = api->mmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == (void *)-1)
        return -1;
    emit(p, v1);
    if (api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0)
        return -2;
    unsigned a = ((gen_fn)p)();
    if (api->munmap(p, PAGE) != 0)
        return -3;
    void *q = api->mmap(p, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (q != p)
        return -4;
    emit(q, v2);
    if (api->mprotect(q, PAGE, PROT_READ | PROT_EXEC) != 0)
        return -5;
    unsigned b = ((gen_fn)q)();
    api->munmap(q, PAGE);
    return ((long)a << 16) | b;
}

/* The pieces, for a test that does them from two threads. */
void *guest_code_map(const struct code_api *api, unsigned v) {
    void *p = api->mmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == (void *)-1)
        return 0;
    emit(p, v);
    if (api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0)
        return 0;
    return p;
}

long guest_code_patch(const struct code_api *api, void *p, unsigned v) {
    if (api->mprotect(p, PAGE, PROT_READ | PROT_WRITE) != 0)
        return -1;
    emit(p, v);
    return api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0 ? -2 : 0;
}

long guest_code_replace(const struct code_api *api, void *p, unsigned v) {
    if (api->munmap(p, PAGE) != 0)
        return -1;
    if (api->mmap(p, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) != p)
        return -2;
    emit(p, v);
    return api->mprotect(p, PAGE, PROT_READ | PROT_EXEC) != 0 ? -3 : 0;
}

long guest_code_call(gen_fn f) { return f(); }

/* Calls `f`, publishes what it returned in flags[0], then spins in
 * translated code -- no host call -- until flags[1] is set, and calls `f`
 * again. The second call is the one that must see the rewritten code. */
long guest_code_call_spin_call(gen_fn f, volatile long *flags) {
    long a = f();
    __atomic_store_n(&flags[0], a, __ATOMIC_RELEASE);
    while (__atomic_load_n(&flags[1], __ATOMIC_ACQUIRE) == 0)
        ;
    return f();
}

/* The same, but waiting inside a host call (`wait`), as a thread blocked in
 * a futex or a condition variable does. */
long guest_code_call_wait_call(gen_fn f, volatile long *flags, void (*wait)(volatile long *)) {
    long a = f();
    __atomic_store_n(&flags[0], a, __ATOMIC_RELEASE);
    wait(flags);
    return f();
}
