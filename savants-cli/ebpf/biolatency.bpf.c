// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: biolatency - block I/O latency histogram
//
// Tracks I/O request start time via block_rq_issue, computes latency
// at block_rq_complete. Aggregates into log2 histogram buckets in-kernel.
// Userspace reads a 32-slot histogram: bucket[i] = count of I/Os with
// latency in range [2^i, 2^(i+1)) microseconds.

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef long long __s64;
typedef int __s32;

// Track I/O start times: key = request pointer, value = start timestamp
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 10240);
    __type(key, __u64);       // request address
    __type(value, __u64);     // start time (ns)
} io_start SEC(".maps");

// Latency histogram: 32 log2 buckets (0-31)
// bucket[0] = <1us, bucket[10] = ~1ms, bucket[20] = ~1s
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 32);
    __type(key, __u32);
    __type(value, __u64);
} io_hist SEC(".maps");

// block_rq_issue: I/O request submitted to device
// We need the request pointer to correlate with completion
// Format: common(8) + dev(4) + pad(4) + sector(8) + nr_sector(4) + ...
struct block_rq_issue_args {
    __u64 __pad;
    __u32 dev;           // offset 8
    __u32 __pad2;        // offset 12
    __u64 sector;        // offset 16
    __u32 nr_sector;     // offset 24
};

SEC("tracepoint/block/block_rq_issue")
int trace_rq_issue(struct block_rq_issue_args *ctx)
{
    __u64 ts = bpf_ktime_get_ns();
    // Use sector as a proxy key since we can't get the request pointer from tracepoint
    // Combine dev + sector for uniqueness
    __u64 key = ((__u64)ctx->dev << 32) | (ctx->sector & 0xFFFFFFFF);
    bpf_map_update_elem(&io_start, &key, &ts, BPF_ANY);
    return 0;
}

// block_rq_complete: I/O request completed
// Format: common(8) + dev(4) + pad(4) + sector(8) + nr_sector(4) + error(4) + ...
struct block_rq_complete_args {
    __u64 __pad;
    __u32 dev;           // offset 8
    __u32 __pad2;        // offset 12
    __u64 sector;        // offset 16
    __u32 nr_sector;     // offset 24
    __s32 error;         // offset 28
};

SEC("tracepoint/block/block_rq_complete")
int trace_rq_complete(struct block_rq_complete_args *ctx)
{
    __u64 key = ((__u64)ctx->dev << 32) | (ctx->sector & 0xFFFFFFFF);

    __u64 *start_ts = bpf_map_lookup_elem(&io_start, &key);
    if (!start_ts)
        return 0;

    __u64 delta = bpf_ktime_get_ns() - *start_ts;
    bpf_map_delete_elem(&io_start, &key);

    // Convert to microseconds and compute log2 bucket
    __u64 us = delta / 1000;
    __u32 bucket = 0;
    __u64 val = us;
    // Manual log2 (BPF doesn't have log functions)
    #pragma unroll
    for (int i = 0; i < 31; i++) {
        if (val > 1) {
            val >>= 1;
            bucket++;
        }
    }
    if (bucket > 31) bucket = 31;

    __u64 *count = bpf_map_lookup_elem(&io_hist, &bucket);
    if (count)
        __sync_fetch_and_add(count, 1);

    return 0;
}

char _license[] SEC("license") = "GPL";
