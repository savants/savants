// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: runqlat - CPU scheduler run queue latency histogram
//
// THE probe with NO /proc fallback. Measures how long tasks wait
// for CPU after being woken up. High runqlat = CPU saturation even
// when utilization looks normal.
//
// Records timestamp at sched_wakeup, computes delta at sched_switch.
// Aggregates into log2 histogram buckets in-kernel.

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;
typedef long __s64;

// Track wakeup times: key = pid, value = wakeup timestamp
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 10240);
    __type(key, __u32);       // pid
    __type(value, __u64);     // wakeup time (ns)
} wakeup_ts SEC(".maps");

// Latency histogram: 32 log2 buckets in microseconds
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 32);
    __type(key, __u32);
    __type(value, __u64);
} runq_hist SEC(".maps");

// sched_wakeup: task is placed on run queue
// Format: common(8) + comm[16](8-24) + pid(24) + prio(28) + target_cpu(32)
struct sched_wakeup_args {
    __u64 __pad;
    char comm[16];       // offset 8
    __s32 pid;           // offset 24
    __s32 prio;          // offset 28
    __s32 target_cpu;    // offset 32
};

SEC("tracepoint/sched/sched_wakeup")
int trace_wakeup(struct sched_wakeup_args *ctx)
{
    __u32 pid = ctx->pid;
    __u64 ts = bpf_ktime_get_ns();
    bpf_map_update_elem(&wakeup_ts, &pid, &ts, BPF_ANY);
    return 0;
}

// sched_switch: task is scheduled onto CPU
// Format: common(8) + prev_comm[16](8-24) + prev_pid(24) + prev_prio(28)
//         + prev_state(32-40) + next_comm[16](40-56) + next_pid(56)
struct sched_switch_args {
    __u64 __pad;
    char prev_comm[16];  // offset 8
    __s32 prev_pid;      // offset 24
    __s32 prev_prio;     // offset 28
    __s64 prev_state;    // offset 32
    char next_comm[16];  // offset 40
    __s32 next_pid;      // offset 56
};

SEC("tracepoint/sched/sched_switch")
int trace_switch(struct sched_switch_args *ctx)
{
    __u32 pid = ctx->next_pid;

    __u64 *wake_ts = bpf_map_lookup_elem(&wakeup_ts, &pid);
    if (!wake_ts)
        return 0;

    __u64 delta = bpf_ktime_get_ns() - *wake_ts;
    bpf_map_delete_elem(&wakeup_ts, &pid);

    // Convert to microseconds and compute log2 bucket
    __u64 us = delta / 1000;
    __u32 bucket = 0;
    __u64 val = us;
    #pragma unroll
    for (int i = 0; i < 31; i++) {
        if (val > 1) {
            val >>= 1;
            bucket++;
        }
    }
    if (bucket > 31) bucket = 31;

    __u64 *count = bpf_map_lookup_elem(&runq_hist, &bucket);
    if (count)
        __sync_fetch_and_add(count, 1);

    return 0;
}

char _license[] SEC("license") = "GPL";
