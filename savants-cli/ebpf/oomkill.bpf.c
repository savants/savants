// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: oomkill - OOM killer events with full victim details
//
// Fires on oom/mark_victim tracepoint. Captures the victim PID,
// comm, memory stats (total_vm, anon_rss, file_rss, shmem_rss),
// and UID. Near-zero overhead - only fires during OOM events.

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

struct oom_event {
    __u64 ts;
    __s32 pid;
    __u32 uid;
    __u64 total_vm;      // total virtual memory (pages)
    __u64 anon_rss;      // anonymous RSS (pages)
    __u64 file_rss;      // file-backed RSS (pages)
    __u64 shmem_rss;     // shared memory RSS (pages)
};

// Last 32 OOM events
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 32);
    __type(key, __u32);
    __type(value, struct oom_event);
} oom_events SEC(".maps");

// Write index
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} oom_idx SEC(".maps");

// Total OOM kill count
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} oom_total SEC(".maps");

// oom/mark_victim format:
//   field:int pid;               offset:8;  size:4;
//   field:__data_loc char[] comm; offset:12; size:4;
//   field:unsigned long total_vm; offset:16; size:8;
//   field:unsigned long anon_rss; offset:24; size:8;
//   field:unsigned long file_rss; offset:32; size:8;
//   field:unsigned long shmem_rss; offset:40; size:8;
//   field:uid_t uid;             offset:48; size:4;
//   field:unsigned long pgtables; offset:56; size:8;
struct oom_args {
    __u64 __pad;
    __s32 pid;           // offset 8
    __u32 comm_loc;      // offset 12 (__data_loc)
    __u64 total_vm;      // offset 16
    __u64 anon_rss;      // offset 24
    __u64 file_rss;      // offset 32
    __u64 shmem_rss;     // offset 40
    __u32 uid;           // offset 48
};

SEC("tracepoint/oom/mark_victim")
int trace_oom(struct oom_args *ctx)
{
    struct oom_event evt = {};
    evt.ts = bpf_ktime_get_ns();
    evt.pid = ctx->pid;
    evt.uid = ctx->uid;
    evt.total_vm = ctx->total_vm;
    evt.anon_rss = ctx->anon_rss;
    evt.file_rss = ctx->file_rss;
    evt.shmem_rss = ctx->shmem_rss;

    // Increment total
    __u32 zero = 0;
    __u64 *total = bpf_map_lookup_elem(&oom_total, &zero);
    if (total)
        __sync_fetch_and_add(total, 1);

    // Write to ring
    __u64 *idx = bpf_map_lookup_elem(&oom_idx, &zero);
    if (idx) {
        __u32 slot = ((__u32)(*idx)) & 31; // mod 32
        bpf_map_update_elem(&oom_events, &slot, &evt, BPF_ANY);
        __sync_fetch_and_add(idx, 1);
    }

    return 0;
}

char _license[] SEC("license") = "GPL";
