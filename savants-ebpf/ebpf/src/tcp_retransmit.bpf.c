// SPDX-License-Identifier: GPL-2.0
// Savants eBPF probe: tcp_retransmit_skb
//
// Self-contained - no external headers needed.
// Compile: clang -O2 -g -target bpf -c tcp_retransmit.bpf.c -o tcp_retransmit.bpf.o
//
// Attaches to tracepoint/tcp/tcp_retransmit_skb
// Aggregates retransmit counts per destination IP in a BPF hash map.

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

// BPF helper function signatures
static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;
static long (*bpf_map_update_elem)(void *map, const void *key, const void *value, __u64 flags) = (void *)2;
static __u64 (*bpf_ktime_get_ns)(void) = (void *)5;

#define SEC(name) __attribute__((section(name), used))
#define BPF_ANY 0

// BPF map definitions (BTF-style)
struct {
    int (*type)[1];       // BPF_MAP_TYPE_HASH = 1
    int (*max_entries)[1024];
    __u32 *key;
    __u64 *value;
} retransmits SEC(".maps");

struct {
    int (*type)[2];       // BPF_MAP_TYPE_ARRAY = 2
    int (*max_entries)[1];
    __u32 *key;
    __u64 *value;
} total_retrans SEC(".maps");

// Tracepoint context for tcp/tcp_retransmit_skb
// Field offsets from /sys/kernel/tracing/events/tcp/tcp_retransmit_skb/format:
//   field:__u16 sport;  offset:28; size:2;
//   field:__u16 dport;  offset:30; size:2;
//   field:__u16 family; offset:32; size:2;
//   field:__u8 saddr[4]; offset:34; size:4;
//   field:__u8 daddr[4]; offset:38; size:4;
struct tp_args {
    __u64 _pad0;         // offset 0-7: common fields
    __u64 skbaddr;       // offset 8
    __u64 skaddr;        // offset 16
    __s32 state;         // offset 24
    __u16 sport;         // offset 28
    __u16 dport;         // offset 30
    __u16 family;        // offset 32
    __u8 saddr[4];       // offset 34
    __u8 daddr[4];       // offset 38
};

SEC("tracepoint/tcp/tcp_retransmit_skb")
int trace_tcp_retransmit(struct tp_args *ctx)
{
    // Only IPv4 (family == 2)
    if (ctx->family != 2)
        return 0;

    // Use destination IP as the map key
    __u32 daddr;
    __builtin_memcpy(&daddr, ctx->daddr, 4);

    // Look up existing count for this destination
    __u64 *count = bpf_map_lookup_elem(&retransmits, &daddr);
    if (count) {
        __sync_fetch_and_add(count, 1);
    } else {
        __u64 one = 1;
        bpf_map_update_elem(&retransmits, &daddr, &one, BPF_ANY);
    }

    // Increment total counter
    __u32 zero = 0;
    __u64 *total = bpf_map_lookup_elem(&total_retrans, &zero);
    if (total) {
        __sync_fetch_and_add(total, 1);
    }

    return 0;
}

char _license[] SEC("license") = "GPL";
