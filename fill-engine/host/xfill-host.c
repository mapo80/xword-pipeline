/*
 * xfill-host: loads the compiled engine library (libxfill) in a child
 * process and forwards its C interface (include/xfill.h) over stdin/stdout.
 * A native fault in the engine (abort, invalid memory access, stack
 * overflow, failed allocation) ends this process, never the application
 * that started it, which sees the pipe close.
 *
 *   xfill-host <path of libxfill.so | libxfill.dylib | xfill.dll>
 *
 * Protocol (integers little-endian):
 *   request:  kind u8, id u32, len u32, payload[len]
 *   response: kind u8, id u32, rc i32, len_a u32, a[len_a], len_b u32, b[len_b]
 * On start: HELLO (id 0, a = engine version). Requests:
 *   BUILD     csv path NUL dictionary path   -> a = stats JSON, b = error
 *   OPEN      dictionary path                -> a = {"dict": n, stats...}
 *   CLOSE     dict u32 (freed once its generations end)
 *   GENERATE  dict u32, request JSON         -> a = result JSON, b = error
 *   CANCEL    id = the generation to stop    -> no response
 *   VALIDATE  grid JSON                      -> a = report JSON
 *   FAULT     abort | segv | overflow        -> (tests only: needs the
 *             environment XFILL_HOST_FAULT_INJECTION=1)
 * Return codes are those of xfill.h. Generations run on their own threads
 * (several at once; CANCEL sets their cancel int). Dictionary files are
 * opened by the library, which maps them: pages shared and reclaimable.
 * CSV files are mapped too (no copy). The library's stdout goes to stderr:
 * stdout carries only the protocol. On Linux the process volunteers as
 * the first victim of the out-of-memory killer (oom_score_adj 1000), so a
 * memory shortage ends the engine, not the application. The process exits
 * when stdin closes.
 */
#ifndef _WIN32
#define _POSIX_C_SOURCE 200809L /* POSIX declarations under -std=c11 (glibc) */
#endif
#ifdef _WIN32
#define _CRT_SECURE_NO_WARNINGS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <process.h>
#include <io.h>
#include <fcntl.h>
#else
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#endif
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum {
    HELLO = 0,
    OPEN = 1,
    GENERATE = 2,
    CANCEL = 3,
    VALIDATE = 4,
    BUILD = 5,
    CLOSE = 6,
    FAULT = 0x7F
};
enum { OK = 0, ERR_ARG = -1, ERR_IO = -7 };
#define MAX_PAYLOAD (1u << 30)
#define GENERATE_STACK (8u << 20) /* the solver recurses: a roomy stack */

typedef struct XfillWordlist XfillWordlist;
typedef const char *(*version_fn)(void);
typedef int (*build_fn)(const uint8_t *, size_t, const char *, char **, char **);
typedef int (*open_fn)(const char *, XfillWordlist **, char **);
typedef void (*free_fn)(XfillWordlist *);
typedef int (*stats_fn)(const XfillWordlist *, char **);
typedef int (*generate_fn)(const XfillWordlist *, const char *, const volatile int32_t *, char **, char **);
typedef int (*validate_fn)(const char *, char **);
typedef void (*string_free_fn)(char *);

static version_fn x_version;
static build_fn x_build;
static open_fn x_open;
static free_fn x_free;
static stats_fn x_stats;
static generate_fn x_generate;
static validate_fn x_validate;
static string_free_fn x_string_free;

/* ---- platform: pipes, locks, threads, atomics -------------------------- */

#ifdef _WIN32
static HANDLE in_handle, out_handle;
static CRITICAL_SECTION out_lock, state_lock;
#define LOCK(l) EnterCriticalSection(&(l))
#define UNLOCK(l) LeaveCriticalSection(&(l))
static void set_flag(volatile int32_t *p) { InterlockedExchange((volatile LONG *)p, 1); }
#else
static int in_fd, out_fd;
static pthread_mutex_t out_lock = PTHREAD_MUTEX_INITIALIZER, state_lock = PTHREAD_MUTEX_INITIALIZER;
#define LOCK(l) pthread_mutex_lock(&(l))
#define UNLOCK(l) pthread_mutex_unlock(&(l))
static void set_flag(volatile int32_t *p) { __atomic_store_n(p, 1, __ATOMIC_SEQ_CST); }
#endif

static void die(const char *what, const char *detail)
{
    fprintf(stderr, "xfill-host: %s%s%s\n", what, detail ? ": " : "", detail ? detail : "");
    fflush(stderr);
    exit(3);
}

/* Read exactly n bytes; 0 at end of input. */
static int read_all(void *buf, size_t n)
{
    char *p = buf;
    while (n > 0) {
#ifdef _WIN32
        DWORD got = 0;
        DWORD want = n > 0x40000000u ? 0x40000000u : (DWORD)n;
        if (!ReadFile(in_handle, p, want, &got, NULL) || got == 0)
            return 0;
#else
        ssize_t got = read(in_fd, p, n);
        if (got <= 0)
            return 0;
#endif
        p += got;
        n -= (size_t)got;
    }
    return 1;
}

static void write_all(const void *buf, size_t n)
{
    const char *p = buf;
    while (n > 0) {
#ifdef _WIN32
        DWORD put = 0;
        if (!WriteFile(out_handle, p, (DWORD)n, &put, NULL) || put == 0)
            return; /* the application is gone: nothing left to tell it */
#else
        ssize_t put = write(out_fd, p, n);
        if (put <= 0)
            return;
#endif
        p += put;
        n -= (size_t)put;
    }
}

static void put_u32(unsigned char *p, uint32_t v)
{
    p[0] = (unsigned char)v;
    p[1] = (unsigned char)(v >> 8);
    p[2] = (unsigned char)(v >> 16);
    p[3] = (unsigned char)(v >> 24);
}

static uint32_t get_u32(const unsigned char *p)
{
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

/* One response frame, written with a single call under the lock. */
static void respond(uint8_t kind, uint32_t id, int32_t rc, const char *a, const char *b)
{
    size_t la = a ? strlen(a) : 0, lb = b ? strlen(b) : 0;
    size_t total = 17 + la + lb;
    unsigned char *frame = malloc(total);
    if (!frame)
        die("out of memory", NULL);
    frame[0] = kind;
    put_u32(frame + 1, id);
    put_u32(frame + 5, (uint32_t)rc);
    put_u32(frame + 9, (uint32_t)la);
    if (la)
        memcpy(frame + 13, a, la);
    put_u32(frame + 13 + la, (uint32_t)lb);
    if (lb)
        memcpy(frame + 17 + la, b, lb);
    LOCK(out_lock);
    write_all(frame, total);
    UNLOCK(out_lock);
    free(frame);
}

/* Respond with library strings, then free them. */
static void respond_owned(uint8_t kind, uint32_t id, int32_t rc, char *a, char *b)
{
    respond(kind, id, rc, a, b);
    if (a)
        x_string_free(a);
    if (b)
        x_string_free(b);
}

/* ---- open dictionaries and generations (guarded by state_lock) ---------- */

typedef struct Dict {
    uint32_t id;
    XfillWordlist *handle;
    int users;  /* generations in progress */
    int closed; /* freed when the last user ends */
    struct Dict *next;
} Dict;

typedef struct Job {
    uint32_t id;
    volatile int32_t cancel;
    Dict *dict;
    char *json;
    struct Job *next;
} Job;

static Dict *dicts;
static uint32_t next_dict = 1;
static Job *jobs;

/* The open dictionary `id` with one more user, or NULL. */
static Dict *dict_take(uint32_t id)
{
    LOCK(state_lock);
    Dict *d = dicts;
    while (d && (d->id != id || d->closed))
        d = d->next;
    if (d)
        d->users++;
    UNLOCK(state_lock);
    return d;
}

/* Under the lock: unlink `d` if closed and unused; returns the handle to
 * free (outside the lock), else NULL. */
static XfillWordlist *dict_unlink_if_done(Dict *d)
{
    if (!d->closed || d->users > 0)
        return NULL;
    for (Dict **p = &dicts; *p; p = &(*p)->next) {
        if (*p == d) {
            *p = d->next;
            break;
        }
    }
    XfillWordlist *h = d->handle;
    free(d);
    return h;
}

static void dict_release(Dict *d)
{
    LOCK(state_lock);
    d->users--;
    XfillWordlist *h = dict_unlink_if_done(d);
    UNLOCK(state_lock);
    if (h)
        x_free(h);
}

static void job_remove(Job *job)
{
    LOCK(state_lock);
    for (Job **p = &jobs; *p; p = &(*p)->next) {
        if (*p == job) {
            *p = job->next;
            break;
        }
    }
    UNLOCK(state_lock);
}

static void cancel_job(uint32_t id, int all)
{
    LOCK(state_lock);
    for (Job *j = jobs; j; j = j->next)
        if (all || j->id == id)
            set_flag(&j->cancel);
    UNLOCK(state_lock);
}

static void run_job(Job *job)
{
    char *result = NULL, *err = NULL;
    int rc = x_generate(job->dict->handle, job->json, &job->cancel, &result, &err);
    job_remove(job);
    dict_release(job->dict);
    respond_owned(GENERATE, job->id, rc, result, err);
    free(job->json);
    free(job);
}

#ifdef _WIN32
static unsigned __stdcall job_thread(void *arg)
{
    run_job(arg);
    return 0;
}
#else
static void *job_thread(void *arg)
{
    run_job(arg);
    return NULL;
}
#endif

/* payload: dict u32, request JSON (NUL-terminated by the reader); takes
 * ownership of the payload */
static void start_generation(uint32_t id, char *payload, uint32_t len)
{
    Dict *d = len >= 4 ? dict_take(get_u32((unsigned char *)payload)) : NULL;
    if (!d) {
        respond(GENERATE, id, ERR_ARG, NULL, "no such dictionary");
        free(payload);
        return;
    }
    Job *job = calloc(1, sizeof *job);
    char *json = malloc(len - 4 + 1);
    if (!job || !json)
        die("out of memory", NULL);
    memcpy(json, payload + 4, len - 4 + 1);
    free(payload);
    job->id = id;
    job->dict = d;
    job->json = json;
    LOCK(state_lock);
    job->next = jobs; /* registered before the thread: a CANCEL read next finds it */
    jobs = job;
    UNLOCK(state_lock);
#ifdef _WIN32
    uintptr_t t = _beginthreadex(NULL, GENERATE_STACK, job_thread, job, STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    if (t == 0)
        die("cannot start a thread", NULL);
    CloseHandle((HANDLE)t);
#else
    pthread_attr_t attr;
    pthread_t t;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, GENERATE_STACK);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    if (pthread_create(&t, &attr, job_thread, job) != 0)
        die("cannot start a thread", NULL);
    pthread_attr_destroy(&attr);
#endif
}

/* ---- requests ---------------------------------------------------------- */

/* A file mapped read-only (no copy: its pages are clean and go with the
 * mapping). */
typedef struct {
    const uint8_t *bytes;
    size_t len;
#ifdef _WIN32
    HANDLE file, map;
#else
    int fd;
#endif
} Mapped;

static int map_file(const char *path, Mapped *m)
{
    memset(m, 0, sizeof *m);
#ifdef _WIN32
    wchar_t wpath[32768];
    LARGE_INTEGER size;
    m->file = INVALID_HANDLE_VALUE;
    if (MultiByteToWideChar(CP_UTF8, 0, path, -1, wpath, 32768))
        m->file = CreateFileW(wpath, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (m->file == INVALID_HANDLE_VALUE)
        return 0;
    if (!GetFileSizeEx(m->file, &size)) {
        CloseHandle(m->file);
        return 0;
    }
    m->len = (size_t)size.QuadPart;
    if (m->len > 0) {
        m->map = CreateFileMappingW(m->file, NULL, PAGE_READONLY, 0, 0, NULL);
        m->bytes = m->map ? MapViewOfFile(m->map, FILE_MAP_READ, 0, 0, 0) : NULL;
        if (!m->bytes) {
            if (m->map)
                CloseHandle(m->map);
            CloseHandle(m->file);
            return 0;
        }
    }
#else
    struct stat st;
    m->fd = open(path, O_RDONLY);
    if (m->fd < 0)
        return 0;
    if (fstat(m->fd, &st) != 0) {
        close(m->fd);
        return 0;
    }
    m->len = (size_t)st.st_size;
    if (m->len > 0) {
        void *p = mmap(NULL, m->len, PROT_READ, MAP_PRIVATE, m->fd, 0);
        if (p == MAP_FAILED) {
            close(m->fd);
            return 0;
        }
        m->bytes = p;
    }
#endif
    return 1;
}

static void unmap_file(Mapped *m)
{
#ifdef _WIN32
    if (m->bytes) {
        UnmapViewOfFile(m->bytes);
        CloseHandle(m->map);
    }
    CloseHandle(m->file);
#else
    if (m->bytes)
        munmap((void *)m->bytes, m->len);
    close(m->fd);
#endif
}

/* payload: csv path NUL dictionary path */
static void build(uint32_t id, const char *payload, uint32_t len)
{
    const char *csv_path = payload;
    size_t first = strlen(csv_path);
    if (first + 1 >= len) {
        respond(BUILD, id, ERR_ARG, NULL, "expected: csv path, NUL, dictionary path");
        return;
    }
    const char *out_path = payload + first + 1;
    Mapped csv;
    if (!map_file(csv_path, &csv)) {
        char msg[256];
        snprintf(msg, sizeof msg, "cannot read %.200s", csv_path);
        respond(BUILD, id, ERR_IO, NULL, msg);
        return;
    }
    char *stats = NULL, *err = NULL;
    int rc = x_build(csv.bytes ? csv.bytes : (const uint8_t *)"", csv.len, out_path, &stats, &err);
    unmap_file(&csv);
    respond_owned(BUILD, id, rc, stats, err);
}

static void open_dict(uint32_t id, const char *path)
{
    XfillWordlist *handle = NULL;
    char *err = NULL, *stats = NULL;
    int rc = x_open(path, &handle, &err);
    if (rc != OK) {
        respond_owned(OPEN, id, rc, NULL, err);
        return;
    }
    rc = x_stats(handle, &stats);
    Dict *d = calloc(1, sizeof *d);
    if (!d)
        die("out of memory", NULL);
    d->handle = handle;
    LOCK(state_lock);
    d->id = next_dict++;
    d->next = dicts;
    dicts = d;
    UNLOCK(state_lock);
    /* {"dict":n, then the stats object without its opening brace */
    size_t ls = stats ? strlen(stats) : 0;
    char *reply = malloc(ls + 32);
    if (!reply)
        die("out of memory", NULL);
    snprintf(reply, ls + 32, "{\"dict\":%u%s%s", (unsigned)d->id, ls > 2 ? "," : "}", ls > 2 ? stats + 1 : "");
    respond(OPEN, id, rc, reply, NULL);
    free(reply);
    if (stats)
        x_string_free(stats);
}

static void close_dict(uint32_t id, const char *payload, uint32_t len)
{
    uint32_t dict = len >= 4 ? get_u32((const unsigned char *)payload) : 0;
    XfillWordlist *h = NULL;
    int found = 0;
    LOCK(state_lock);
    for (Dict *d = dicts; d; d = d->next) {
        if (d->id == dict && !d->closed) {
            d->closed = 1;
            found = 1;
            h = dict_unlink_if_done(d);
            break;
        }
    }
    UNLOCK(state_lock);
    if (h)
        x_free(h);
    respond(CLOSE, id, found ? OK : ERR_ARG, NULL, found ? NULL : "no such dictionary");
}

static void validate(uint32_t id, const char *json)
{
    char *report = NULL;
    int rc = x_validate(json, &report);
    respond_owned(VALIDATE, id, rc, report, NULL);
}

#if defined(__clang__) || defined(__GNUC__)
__attribute__((noinline))
#endif
static int deep(volatile int n)
{
    volatile char pad[4096];
    pad[n & 4095] = (char)n;
    if (n < 0) /* never: n only grows (keeps the recursion legal C) */
        return 0;
    return deep(n + 1) + pad[7];
}

static void fault(uint32_t id, const char *kind)
{
    if (strcmp(kind, "abort") == 0)
        abort();
    if (strcmp(kind, "segv") == 0) {
        volatile uintptr_t invalid = 8; /* opaque to the compiler */
        *(volatile int *)invalid = 1;
    }
    if (strcmp(kind, "overflow") == 0)
        respond(FAULT, id, deep(0), NULL, NULL);
    respond(FAULT, id, ERR_ARG, NULL, "unknown fault");
}

/* ---- start ------------------------------------------------------------- */

/* Resolve `name` into the function pointer at `fn` (memcpy: portable between
 * data and function pointers, also for MSVC /W4). */
#ifdef _WIN32
static void symbol(HMODULE lib, const char *name, void *fn)
{
    FARPROC p = GetProcAddress(lib, name);
    if (!p)
        die("missing symbol", name);
    memcpy(fn, &p, sizeof p);
}
#else
static void symbol(void *lib, const char *name, void *fn)
{
    void *p = dlsym(lib, name);
    if (!p)
        die("missing symbol", name);
    memcpy(fn, &p, sizeof p);
}
#endif

#ifdef __linux__
/* The first victim of the out-of-memory killer: the engine, not the app. */
static void volunteer_for_oom(void)
{
    FILE *f = fopen("/proc/self/oom_score_adj", "w");
    if (f) {
        fputs("1000", f);
        fclose(f);
    }
}
#endif

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
#else
int main(int argc, char **argv)
#endif
{
    if (argc != 2)
        die("usage: xfill-host <engine library>", NULL);
    /* stdout is the protocol: the library's own output goes to stderr */
#ifdef _WIN32
    in_handle = GetStdHandle(STD_INPUT_HANDLE);
    /* a private copy of the protocol pipe: redirecting the CRT's stdout below
     * closes the original handle */
    if (!DuplicateHandle(GetCurrentProcess(), GetStdHandle(STD_OUTPUT_HANDLE), GetCurrentProcess(), &out_handle, 0,
                         FALSE, DUPLICATE_SAME_ACCESS))
        die("cannot duplicate stdout", NULL);
    SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
    _dup2(_fileno(stderr), _fileno(stdout));
    InitializeCriticalSection(&out_lock);
    InitializeCriticalSection(&state_lock);
    HMODULE lib = LoadLibraryExW(argv[1], NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!lib)
        die("cannot load the engine library", NULL);
#else
    in_fd = 0;
    out_fd = dup(1);
    if (out_fd < 0 || dup2(2, 1) < 0)
        die("cannot redirect stdout", NULL);
#ifdef __linux__
    volunteer_for_oom();
#endif
    void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!lib)
        die("cannot load the engine library", dlerror());
#endif
    symbol(lib, "xfill_version", &x_version);
    symbol(lib, "xfill_dict_build", &x_build);
    symbol(lib, "xfill_dict_open", &x_open);
    symbol(lib, "xfill_wordlist_free", &x_free);
    symbol(lib, "xfill_wordlist_stats", &x_stats);
    symbol(lib, "xfill_generate", &x_generate);
    symbol(lib, "xfill_validate", &x_validate);
    symbol(lib, "xfill_string_free", &x_string_free);
    const char *faults = getenv("XFILL_HOST_FAULT_INJECTION");
    int faults_on = faults && strcmp(faults, "1") == 0;

    respond(HELLO, 0, OK, x_version(), NULL);
    unsigned char head[9];
    while (read_all(head, sizeof head)) {
        uint32_t id = get_u32(head + 1), len = get_u32(head + 5);
        if (len > MAX_PAYLOAD)
            die("payload too large", NULL);
        char *payload = malloc((size_t)len + 1);
        if (!payload)
            die("out of memory", NULL);
        if (!read_all(payload, len))
            break;
        payload[len] = '\0';
        switch (head[0]) {
        case GENERATE:
            start_generation(id, payload, len); /* takes the payload */
            continue;
        case OPEN:
            open_dict(id, payload);
            break;
        case BUILD:
            build(id, payload, len);
            break;
        case CLOSE:
            close_dict(id, payload, len);
            break;
        case CANCEL:
            cancel_job(id, 0);
            break;
        case VALIDATE:
            validate(id, payload);
            break;
        case FAULT:
            if (faults_on)
                fault(id, payload);
            else
                respond(FAULT, id, ERR_ARG, NULL, "fault injection is off");
            break;
        default:
            respond(head[0], id, ERR_ARG, NULL, "unknown request");
            break;
        }
        free(payload);
    }
    /* the application closed the pipe: stop and leave */
    cancel_job(0, 1);
    fflush(stderr);
    _Exit(0);
}
