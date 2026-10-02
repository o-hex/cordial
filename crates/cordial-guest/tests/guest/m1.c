/* Guest code for the M0/M1 tests, compiled for aarch64 by build.rs.
 *
 * Every host function arrives as a pointer argument -- in the tests, the
 * address of a Cordial stub -- so the image has no imports and no relocations
 * and can be mapped anywhere page-aligned. The calls through those pointers
 * are ordinary AAPCS64 calls as Clang emits them, variadic ones included,
 * which is the behaviour under test. */

typedef unsigned long size_t;

long guest_add(long a, long b) { return a + b; }

/* A zero word: permanently unallocated in A64, the M0 control. */
__asm__(".globl guest_udf\n.type guest_udf,%function\nguest_udf:\n.word 0\n");

size_t guest_strlen(size_t (*f)(const char *), const char *s) { return f(s); }

/* Mixed integer and floating-point variadic arguments, chosen so that both
 * the integer and the FP sequence overflow onto the guest stack and the
 * overflowed FP argument sits between two overflowed integer ones -- the case
 * where AAPCS64's stack order and SysV's stop agreeing. */
struct fmt_args {
    long a;
    double b;
    int c;
    double d;
    const char *s;
    unsigned e;
    double g;
    long long h;
    double i;
    int j;
    double k, l, m, o, p;
    int q;
    char r;
    short t;
};

#define FMT "%ld|%.17g|%d|%a|%s|%u|%e|%lld|%.3f|%x|%g|%G|%.0f|%+.5e|%.10f|%d|%c|%hd|%5.2f%%|%-8s|%#o|%*d"

const char *guest_fmt(void) { return FMT; }

int guest_snprintf(int (*f)(char *, size_t, const char *, ...), char *buf, size_t n,
                   const struct fmt_args *x) {
    return f(buf, n, FMT, x->a, x->b, x->c, x->d, x->s, x->e, x->g, x->h, x->i, x->j, x->k,
             x->l, x->m, x->o, x->p, x->q, x->r, x->t, x->b, x->s, x->e, x->c, x->q);
}

static int cmp_int(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

void guest_qsort(void (*q)(void *, size_t, size_t, int (*)(const void *, const void *)),
                 int *base, size_t n) {
    q(base, n, sizeof(int), cmp_int);
}

/* A comparator that itself calls back into the host, so a host->guest call
 * nested inside a guest->host call makes a second guest->host call: three
 * levels of re-entry on one thread. */
struct nested_ctx {
    size_t (*strlen_fn)(const char *);
};
static struct nested_ctx *nested;

static int cmp_by_len(const void *a, const void *b) {
    size_t x = nested->strlen_fn(*(const char *const *)a);
    size_t y = nested->strlen_fn(*(const char *const *)b);
    return (x > y) - (x < y);
}

void guest_qsort_nested(void (*q)(void *, size_t, size_t, int (*)(const void *, const void *)),
                        struct nested_ctx *ctx, const char **base, size_t n) {
    nested = ctx;
    q(base, n, sizeof(char *), cmp_by_len);
}

/* LL/SC: with -mno-outline-atomics this is an LDXR/ADD/STXR/CBNZ loop, the
 * same fallback path the engine's outline-atomics helpers take when LSE is
 * not advertised. */
struct inc_args {
    long *counter;
    long iterations;
};

static void *inc_worker(void *p) {
    struct inc_args *a = p;
    for (long i = 0; i < a->iterations; i++)
        __atomic_fetch_add(a->counter, 1, __ATOMIC_RELAXED);
    return (void *)a->iterations;
}

long guest_spawn(int (*create)(unsigned long *, const void *, void *(*)(void *), void *),
                 int (*join)(unsigned long, void **), struct inc_args *args, int threads) {
    unsigned long tid[256];
    long total = 0;
    if (threads > 256)
        return -1;
    for (int t = 0; t < threads; t++)
        if (create(&tid[t], 0, inc_worker, args) != 0)
            return -2;
    for (int t = 0; t < threads; t++) {
        void *r;
        if (join(tid[t], &r) != 0)
            return -3;
        total += (long)r;
    }
    return total;
}

/* Reads TPIDR_EL0 the way bionic's stack protector does. */
unsigned long guest_tpidr(void) {
    unsigned long v;
    __asm__ volatile("mrs %0, tpidr_el0" : "=r"(v));
    return v;
}

/* Nine doubles, then four ints. AAPCS64 keeps all the ints in registers and
 * overflows only the ninth double; SysV overflows the ninth double *and* the
 * fourth int, and the double comes first in argument order. A fixed
 * x6/x7-then-guest-stack mapping puts them the other way round. */
#define FMT9 "%g %g %g %g %g %g %g %g %g|%d %d %d %d"

const char *guest_fmt9(void) { return FMT9; }

int guest_snprintf9(int (*f)(char *, size_t, const char *, ...), char *buf, size_t n,
                    const double *d, const int *i) {
    return f(buf, n, FMT9, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], i[0], i[1],
             i[2], i[3]);
}

/* Calls f() with FPCR set to `fpcr`, restoring it afterwards. */
unsigned long guest_with_fpcr(unsigned long (*f)(void), unsigned long fpcr) {
    unsigned long old, r;
    __asm__ volatile("mrs %0, fpcr" : "=r"(old));
    __asm__ volatile("msr fpcr, %0" ::"r"(fpcr));
    r = f();
    __asm__ volatile("msr fpcr, %0" ::"r"(old));
    return r;
}

/* M3: a JNI-shaped native the host calls through a host entry, with more
   arguments of every width than either ABI has registers for, so integers
   and floats both overflow to the stack, in different orders on each side.
   Every argument's bits are mixed into the result as an integer -- not as
   floating-point arithmetic, which Clang contracts into fused multiply-adds
   on arm64 and so would not compare bit for bit with the host -- and the
   result is returned as a double, through v0. */
#define MIX(v) h = h * 31 + (unsigned long)(v)
static unsigned long fbits(float f) { unsigned int u; __builtin_memcpy(&u, &f, 4); return u; }
static unsigned long dbits(double d) { unsigned long u; __builtin_memcpy(&u, &d, 8); return u; }
double guest_many(long env, long obj, signed char b, unsigned short c, short s, int i, long j,
                  float f, double d, int i2, long j2, float f2, double d2, double d3, double d4,
                  double d5, double d6, double d7, double d8, int i3, long j3) {
    unsigned long h = (unsigned long)env;
    MIX(obj); MIX(b); MIX(c); MIX(s); MIX(i); MIX(j); MIX(fbits(f)); MIX(dbits(d)); MIX(i2); MIX(j2);
    MIX(fbits(f2)); MIX(dbits(d2)); MIX(dbits(d3)); MIX(dbits(d4)); MIX(dbits(d5)); MIX(dbits(d6));
    MIX(dbits(d7)); MIX(dbits(d8)); MIX(i3); MIX(j3);
    double r;
    __builtin_memcpy(&r, &h, 8);
    return r;
}

/* M3: a plain load, for the test that a guest wild pointer faults in
   translated code and still ends the process with SIGSEGV. */
long guest_load(const volatile long *p) { return *p; }
