// SPDX-License-Identifier: GPL-2.0
// Savants eBPF: tcplife - TCP connection lifecycle tracking
//
// Records connection establishment time via tcp_retransmit_synack (SYN-ACK),
// captures duration + ports at tcp_destroy_sock.
// Aggregates: total connections, short-lived count (<1s), by dest port.
//
// "Connection to postgres:5432 lasted 0.1ms, 0 bytes" = connection refused
// "500 connections to redis:6379 in 60s, all <100ms" = connection pool churn

#include "bpf_helpers.h"

typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef int __s32;

// Track connection start times: key = skaddr, value = start timestamp
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 10240);
    __type(key, __u64);       // sk address
    __type(value, __u64);     // start time (ns)
} conn_start SEC(".maps");

// Connection count by destination port
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, __u16);       // dest port
    __type(value, __u64);     // connection count
} conn_by_port SEC(".maps");

// Short-lived connection count (<1 second)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} short_conns SEC(".maps");

// Total connections destroyed
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} conn_total SEC(".maps");

// Duration histogram (log2 buckets in microseconds)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 32);
    __type(key, __u32);
    __type(value, __u64);
} conn_duration SEC(".maps");

// tcp_destroy_sock: connection torn down
// Format: common(8) + skaddr(8,16) + sport(2,16) + dport(2,18)
//         + family(2,20) + saddr(4,22) + daddr(4,26)
struct tcp_destroy_args {
    __u64 __pad;
    __u64 skaddr;        // offset 8
    __u16 sport;         // offset 16
    __u16 dport;         // offset 18
    __u16 family;        // offset 20
    __u8 saddr[4];       // offset 22
    __u8 daddr[4];       // offset 26
};

SEC("tracepoint/tcp/tcp_destroy_sock")
int trace_destroy(struct tcp_destroy_args *ctx)
{
    if (ctx->family != 2) return 0; // IPv4 only

    __u32 zero = 0;

    // Increment total
    __u64 *total = bpf_map_lookup_elem(&conn_total, &zero);
    if (total)
        __sync_fetch_and_add(total, 1);

    // Count by dest port
    __u16 dport = ctx->dport;
    // dport from tracepoint is already in host byte order
    __u64 *port_count = bpf_map_lookup_elem(&conn_by_port, &dport);
    if (port_count) {
        __sync_fetch_and_add(port_count, 1);
    } else {
        __u64 one = 1;
        bpf_map_update_elem(&conn_by_port, &dport, &one, BPF_ANY);
    }

    // Check if we tracked the start time
    __u64 sk = ctx->skaddr;
    __u64 *start = bpf_map_lookup_elem(&conn_start, &sk);
    if (start) {
        __u64 duration_ns = bpf_ktime_get_ns() - *start;
        bpf_map_delete_elem(&conn_start, &sk);

        // Short-lived check (<1 second)
        if (duration_ns < 1000000000ULL) {
            __u64 *sc = bpf_map_lookup_elem(&short_conns, &zero);
            if (sc) __sync_fetch_and_add(sc, 1);
        }

        // Duration histogram (log2 microseconds)
        __u64 us = duration_ns / 1000;
        __u32 bucket = 0;
        __u64 val = us;
        #pragma unroll
        for (int i = 0; i < 31; i++) {
            if (val > 1) { val >>= 1; bucket++; }
        }
        if (bucket > 31) bucket = 31;
        __u64 *cnt = bpf_map_lookup_elem(&conn_duration, &bucket);
        if (cnt) __sync_fetch_and_add(cnt, 1);
    }

    return 0;
}

// Use tcp_retransmit_synack as a proxy for connection start
// (fires when server sends SYN-ACK, meaning a connection is being established)
struct tcp_synack_args {
    __u64 __pad;
    __u64 skaddr;        // offset 8
};

SEC("tracepoint/tcp/tcp_retransmit_synack")
int trace_synack(struct tcp_synack_args *ctx)
{
    __u64 sk = ctx->skaddr;
    __u64 ts = bpf_ktime_get_ns();
    bpf_map_update_elem(&conn_start, &sk, &ts, BPF_ANY);
    return 0;
}

char _license[] SEC("license") = "GPL";
