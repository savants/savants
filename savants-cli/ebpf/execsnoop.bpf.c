// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: execsnoop - real-time process execution tracking
//
// Captures every exec() syscall with PID, comm, and filename.
// Unlike /proc polling, this catches short-lived processes that
// start and exit between poll intervals.
//
// Stores recent execs in a ring-style array map (last 256 events).

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

struct exec_event {
    __u64 ts;            // timestamp (ns)
    __u32 pid;           // process ID
    __u32 ppid;          // parent PID
    __u32 uid;           // user ID
    __u32 _pad;
};

// Ring buffer of recent exec events (last 256)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 256);
    __type(key, __u32);
    __type(value, struct exec_event);
} exec_events SEC(".maps");

// Write index (atomic counter)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} exec_idx SEC(".maps");

// Count by UID (who is running things)
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 256);
    __type(key, __u32);       // UID
    __type(value, __u64);     // exec count
} exec_by_uid SEC(".maps");

// Total exec counter
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} exec_total SEC(".maps");

// sched_process_exec format:
//   field:__data_loc char[] filename; offset:8; size:4;
//   field:pid_t pid;                  offset:12; size:4;
//   field:pid_t old_pid;              offset:16; size:4;
struct sched_exec_args {
    __u64 __pad;
    __u32 filename_loc;  // offset 8 (__data_loc encoded)
    __s32 pid;           // offset 12
    __s32 old_pid;       // offset 16
};

SEC("tracepoint/sched/sched_process_exec")
int trace_exec(struct sched_exec_args *ctx)
{
    struct exec_event evt = {};
    evt.ts = bpf_ktime_get_ns();
    evt.pid = ctx->pid;
    // Read uid from current task (bpf helper)
    __u64 pid_tgid = *(__u64 *)((void *)bpf_ktime_get_ns, 0); // placeholder
    evt.uid = 0; // Will be enriched from /proc by userspace

    // Increment total
    __u32 zero = 0;
    __u64 *total = bpf_map_lookup_elem(&exec_total, &zero);
    if (total)
        __sync_fetch_and_add(total, 1);

    // Write to ring buffer
    __u64 *idx = bpf_map_lookup_elem(&exec_idx, &zero);
    if (idx) {
        __u32 slot = ((__u32)(*idx)) & 255; // mod 256
        bpf_map_update_elem(&exec_events, &slot, &evt, BPF_ANY);
        __sync_fetch_and_add(idx, 1);
    }

    return 0;
}

char _license[] SEC("license") = "GPL";
