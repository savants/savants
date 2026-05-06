// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: tcpdrop - kernel packet drops with reason code
//
// Attaches to skb/kfree_skb tracepoint. Every time the kernel drops
// a packet, this records the reason code. Aggregated by reason in-kernel.
//
// Key reasons to watch:
//   NO_SOCKET(3): nothing listening on that port
//   NETFILTER_DROP(12): firewall blocked it
//   TCP_RESET(45): connection was reset
//   TCP_OLD_ACK(49): stale connection
//   IP_OUTNOROUTES(54): no route to destination
//   NEIGH_FAILED(58): ARP/NDP resolution failed (link issue!)
//   NOMEM(82): out of memory for network buffers

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

// Count drops per reason code
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 256);  // reason codes go up to ~128
    __type(key, __u32);
    __type(value, __u64);
} drop_reasons SEC(".maps");

// Total drop counter
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} drop_total SEC(".maps");

// skb/kfree_skb format:
// common(8) + skbaddr(8,16) + location(8,24) + rx_sk(8,32)
// + protocol(2,32) + pad + reason(4,36)
struct kfree_skb_args {
    __u64 __pad;
    __u64 skbaddr;       // offset 8
    __u64 location;      // offset 16
    __u64 rx_sk;         // offset 24
    __u16 protocol;      // offset 32
    __u16 __pad2;        // offset 34
    __u32 reason;        // offset 36
};

SEC("tracepoint/skb/kfree_skb")
int trace_kfree_skb(struct kfree_skb_args *ctx)
{
    __u32 reason = ctx->reason;

    // Skip reason 0 and 1 (NOT_SPECIFIED and CONSUMED - normal)
    if (reason < 2)
        return 0;

    // Increment per-reason counter
    if (reason < 256) {
        __u64 *count = bpf_map_lookup_elem(&drop_reasons, &reason);
        if (count)
            __sync_fetch_and_add(count, 1);
    }

    // Increment total
    __u32 zero = 0;
    __u64 *total = bpf_map_lookup_elem(&drop_total, &zero);
    if (total)
        __sync_fetch_and_add(total, 1);

    return 0;
}

char _license[] SEC("license") = "GPL";
