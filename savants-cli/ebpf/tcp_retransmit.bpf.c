// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: tcp_retransmit_skb tracepoint
//
// Fires on every TCP retransmit. Aggregates by destination IP.
// If multiple destinations spike = local link/ISP issue.
// Aya-compatible BTF map definitions.

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

// Per-destination retransmit counter
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, __u32);       // destination IPv4 address
    __type(value, __u64);     // retransmit count
} retransmits SEC(".maps");

// Total retransmit counter (single element array)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} total SEC(".maps");

// Tracepoint args: tcp/tcp_retransmit_skb
// Offsets from /sys/kernel/tracing/events/tcp/tcp_retransmit_skb/format
struct tcp_retransmit_args {
    // Common tracepoint header (8 bytes)
    __u64 __pad;
    // Event fields
    const void *skbaddr;     // offset 8
    const void *skaddr;      // offset 16
    __s32 state;             // offset 24
    __u16 sport;             // offset 28
    __u16 dport;             // offset 30
    __u16 family;            // offset 32
    __u8 saddr[4];           // offset 34
    __u8 daddr[4];           // offset 38
};

SEC("tracepoint/tcp/tcp_retransmit_skb")
int trace_retransmit(struct tcp_retransmit_args *ctx)
{
    // IPv4 only (family == 2)
    if (ctx->family != 2)
        return 0;

    // Get destination IP as u32
    __u32 daddr;
    __builtin_memcpy(&daddr, ctx->daddr, 4);

    // Increment per-destination count
    __u64 *count = bpf_map_lookup_elem(&retransmits, &daddr);
    if (count) {
        __sync_fetch_and_add(count, 1);
    } else {
        __u64 one = 1;
        bpf_map_update_elem(&retransmits, &daddr, &one, BPF_ANY);
    }

    // Increment total
    __u32 zero = 0;
    __u64 *tot = bpf_map_lookup_elem(&total, &zero);
    if (tot)
        __sync_fetch_and_add(tot, 1);

    return 0;
}

char _license[] SEC("license") = "GPL";
