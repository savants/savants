// Minimal BPF helpers for Aya-compatible programs
// Only what's needed - no full libbpf dependency

#ifndef __BPF_HELPERS_H
#define __BPF_HELPERS_H

#define SEC(name) __attribute__((section(name), used))

// BPF map type constants
#define BPF_MAP_TYPE_HASH 1
#define BPF_MAP_TYPE_ARRAY 2

// BPF map flags
#define BPF_ANY 0
#define BPF_NOEXIST 1
#define BPF_EXIST 2

// Map definition macros (BTF-style, what Aya expects)
#define __uint(name, val) int (*name)[val]
#define __type(name, val) typeof(val) *name

// BPF helper functions
static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;
static long (*bpf_map_update_elem)(void *map, const void *key, const void *value, unsigned long long flags) = (void *)2;
static long (*bpf_map_delete_elem)(void *map, const void *key) = (void *)3;
static unsigned long long (*bpf_ktime_get_ns)(void) = (void *)5;

#endif
