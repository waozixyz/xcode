// Live-allocation tracer for attributing an Xcoder session's retained heap.
//
// Preloaded into the CLI, it records every allocation of at least
// XCODER_ALLOC_TRACE_MIN bytes (default 2048) with its call stack and forgets
// it when freed. Creating the file "$XCODER_ALLOC_TRACE.trigger" writes the
// live set, grouped by stack, to "$XCODER_ALLOC_TRACE". Other programs that
// inherit the preload pass straight through to the C library.
#define _GNU_SOURCE
#include <execinfo.h>
#include <fcntl.h>
#include <malloc.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void *__libc_memalign(size_t, size_t);
extern void __libc_free(void *);

#define FRAMES 24
#define SLOTS (1u << 21)
#define STACKS (1u << 16)

typedef struct { void *pointer; size_t size; uint32_t stack; } Slot;
typedef struct { void *frames[FRAMES]; int count; uint64_t hash; size_t live_bytes; size_t live_count; size_t total_count; } Stack;

static Slot *slots;
static Stack *stacks;
static uint32_t stack_count;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int active;
static size_t minimum = 2048;
static char output[512];
static __thread int inside;

static uint64_t mix(uint64_t value) {
    value ^= value >> 33; value *= 0xff51afd7ed558ccdull; value ^= value >> 33;
    value *= 0xc4ceb9fe1a85ec53ull; value ^= value >> 33; return value;
}

static void record(void *pointer, size_t size) {
    if (!active || !pointer || size < minimum || inside) return;
    inside = 1;
    void *frames[FRAMES + 2];
    int count = backtrace(frames, FRAMES + 2) - 2;
    if (count < 0) count = 0;
    uint64_t hash = 1469598103934665603ull;
    for (int i = 0; i < count; i++) hash = mix(hash ^ (uintptr_t)frames[i + 2]);
    pthread_mutex_lock(&lock);
    uint32_t index = (uint32_t)(hash % STACKS), found = UINT32_MAX;
    for (uint32_t probe = 0; probe < STACKS; probe++, index = (index + 1) % STACKS) {
        Stack *stack = &stacks[index];
        if (!stack->count && !stack->hash) {
            if (stack_count < STACKS - 1) {
                stack->count = count; stack->hash = hash; stack_count++;
                memcpy(stack->frames, frames + 2, sizeof(void *) * (size_t)count);
                found = index;
            }
            break;
        }
        if (stack->hash == hash && stack->count == count) { found = index; break; }
    }
    if (found != UINT32_MAX) {
        uint32_t slot = (uint32_t)(mix((uintptr_t)pointer) % SLOTS);
        for (uint32_t probe = 0; probe < SLOTS; probe++, slot = (slot + 1) % SLOTS) {
            if (!slots[slot].pointer || slots[slot].pointer == (void *)1) {
                slots[slot].pointer = pointer; slots[slot].size = size; slots[slot].stack = found;
                stacks[found].live_bytes += size; stacks[found].live_count++; stacks[found].total_count++;
                break;
            }
        }
    }
    pthread_mutex_unlock(&lock);
    inside = 0;
}

static void forget(void *pointer) {
    if (!active || !pointer || inside) return;
    pthread_mutex_lock(&lock);
    uint32_t slot = (uint32_t)(mix((uintptr_t)pointer) % SLOTS);
    for (uint32_t probe = 0; probe < SLOTS; probe++, slot = (slot + 1) % SLOTS) {
        if (!slots[slot].pointer) break;
        if (slots[slot].pointer == pointer) {
            stacks[slots[slot].stack].live_bytes -= slots[slot].size;
            stacks[slots[slot].stack].live_count--;
            slots[slot].pointer = (void *)1;
            break;
        }
    }
    pthread_mutex_unlock(&lock);
}

static void dump(void) {
    inside = 1;
    FILE *file = fopen(output, "w");
    if (file) {
        FILE *maps = fopen("/proc/self/maps", "r");
        char line[1024];
        while (maps && fgets(line, sizeof line, maps)) fprintf(file, "M %s", line);
        if (maps) fclose(maps);
        struct mallinfo2 info = mallinfo2();
        fprintf(file, "I heap_bytes=%zu in_use=%zu free_retained=%zu mmapped=%zu trimmable=%zu\n",
                info.arena, info.uordblks, info.fordblks, info.hblkhd, info.keepcost);
        pthread_mutex_lock(&lock);
        for (uint32_t index = 0; index < STACKS; index++) {
            Stack *stack = &stacks[index];
            if (!stack->live_bytes) continue;
            fprintf(file, "S %zu %zu %zu", stack->live_bytes, stack->live_count, stack->total_count);
            for (int i = 0; i < stack->count; i++) fprintf(file, " %p", stack->frames[i]);
            fputc('\n', file);
        }
        pthread_mutex_unlock(&lock);
        fclose(file);
    }
    inside = 0;
}

static void *watch(void *unused) {
    (void)unused;
    inside = 1;
    char trigger[540];
    snprintf(trigger, sizeof trigger, "%s.trigger", output);
    for (;;) {
        struct timespec wait = {0, 200000000};
        nanosleep(&wait, NULL);
        if (access(trigger, F_OK) == 0) { dump(); unlink(trigger); }
    }
    return NULL;
}

__attribute__((constructor)) static void start(void) {
    const char *path = getenv("XCODER_ALLOC_TRACE");
    char executable[512];
    ssize_t length = readlink("/proc/self/exe", executable, sizeof executable - 1);
    if (!path || length <= 0) return;
    executable[length] = 0;
    const char *name = strrchr(executable, '/');
    name = name ? name + 1 : executable;
    if (strcmp(name, "xcoder") && strcmp(name, "xcode") && strcmp(name, "codex")) return;
    if (getenv("XCODER_ALLOC_TRACE_MIN")) minimum = strtoull(getenv("XCODER_ALLOC_TRACE_MIN"), NULL, 10);
    snprintf(output, sizeof output, "%s", path);
    slots = mmap(NULL, sizeof(Slot) * SLOTS, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    stacks = mmap(NULL, sizeof(Stack) * STACKS, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (slots == MAP_FAILED || stacks == MAP_FAILED) return;
    inside = 1;
    void *warm[4];
    backtrace(warm, 4);
    inside = 0;
    pthread_t thread;
    if (pthread_create(&thread, NULL, watch, NULL) == 0) { pthread_detach(thread); active = 1; }
}

void *malloc(size_t size) { void *pointer = __libc_malloc(size); record(pointer, size); return pointer; }
void *calloc(size_t count, size_t size) { void *pointer = __libc_calloc(count, size); record(pointer, count * size); return pointer; }
void *realloc(void *old, size_t size) {
    forget(old);
    void *pointer = __libc_realloc(old, size);
    record(pointer, size);
    return pointer;
}
void free(void *pointer) { forget(pointer); __libc_free(pointer); }
void *memalign(size_t alignment, size_t size) { void *pointer = __libc_memalign(alignment, size); record(pointer, size); return pointer; }
void *aligned_alloc(size_t alignment, size_t size) { return memalign(alignment, size); }
int posix_memalign(void **result, size_t alignment, size_t size) {
    void *pointer = __libc_memalign(alignment, size);
    if (!pointer) return 12;
    record(pointer, size);
    *result = pointer;
    return 0;
}
