// SPDX-License-Identifier: GPL-3.0-or-later
// Test-only clock interposer: simulate suspend without changing host clocks.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

int clock_gettime(clockid_t id, struct timespec *value) {
    static int (*original)(clockid_t, struct timespec *);
    if (!original) {
        original = dlsym(RTLD_NEXT, "clock_gettime");
    }
    int result = original(id, value);
    const char *marker = getenv("TURING_TEST_SUSPEND_FILE");
    if (result == 0 && id == CLOCK_BOOTTIME && marker && access(marker, F_OK) == 0) {
        value->tv_sec += 60;
    }
    return result;
}
