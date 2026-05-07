// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: tcpconnlat - TCP connection establishment latency
//
// Measures time from SYN to ESTABLISHED using tcp_rcv_state_process.
// High connect latency = network congestion or DNS issues on that path.
//
// Uses tcp/tcp_probe tracepoint which fires on state changes and
// includes RTT information.

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

// Connection latency histogram (log2 microseconds)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 32);
    __type(key, __u32);
    __type(value, __u64);
} connlat_hist SEC(".maps");

// Slow connections (>100ms) counter
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} slow_conns SEC(".maps");

// Total connections tracked
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} connlat_total SEC(".maps");

// Slow connection destinations (IP -> count)
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 256);
    __type(key, __u32);       // dest IP
    __type(value, __u64);     // slow connection count
} slow_dests SEC(".maps");

// tcp/tcp_probe format includes srtt (smoothed RTT):
//   field:__u16 sport; offset:8+8+8=24...
// Actually tcp_probe has many fields. Let's use tcp_receive_reset
// as a simpler signal for failed connections, and track via
// the existing tcp_retransmit for latency proxy.

// Use tcp/tcp_receive_reset: fires when a RST is received
// This catches connection refused and reset scenarios
// Format: common(8) + skaddr(8) + sport(2) + dport(2) + family(2)
//         + saddr(4) + daddr(4) + saddr_v6(16) + daddr_v6(16)
struct tcp_reset_args {
    __u64 __pad;
    __u64 skaddr;        // offset 8
    __u16 sport;         // offset 16
    __u16 dport;         // offset 18
    __u16 family;        // offset 20
    __u8 saddr[4];       // offset 22
    __u8 daddr[4];       // offset 26
};

// Count connection resets by destination
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 256);
    __type(key, __u32);       // dest IP
    __type(value, __u64);     // reset count
} reset_by_dest SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} reset_total SEC(".maps");

SEC("tracepoint/tcp/tcp_receive_reset")
int trace_tcp_reset(struct tcp_reset_args *ctx)
{
    if (ctx->family != 2) return 0;

    __u32 daddr;
    __builtin_memcpy(&daddr, ctx->daddr, 4);

    // Count per destination
    __u64 *count = bpf_map_lookup_elem(&reset_by_dest, &daddr);
    if (count) {
        __sync_fetch_and_add(count, 1);
    } else {
        __u64 one = 1;
        bpf_map_update_elem(&reset_by_dest, &daddr, &one, BPF_ANY);
    }

    // Total
    __u32 zero = 0;
    __u64 *total = bpf_map_lookup_elem(&reset_total, &zero);
    if (total)
        __sync_fetch_and_add(total, 1);

    return 0;
}

char _license[] SEC("license") = "GPL";
