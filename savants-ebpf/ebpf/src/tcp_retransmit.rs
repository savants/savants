//! eBPF program: tcp_retransmit_skb tracepoint
//!
//! Attaches to the kernel tcp_retransmit_skb tracepoint.
//! Every time the kernel retransmits a TCP packet, this fires.
//!
//! We aggregate in-kernel using a BPF HashMap:
//!   key = (dest_ip, dest_port, interface_index)
//!   value = count of retransmits
//!
//! Userspace reads the map every N seconds, gets a histogram of
//! retransmits per destination. If ALL destinations spike simultaneously,
//! it's the local link. If one destination spikes, it's that remote server.

#![no_std]
#![no_main]

use aya_ebpf::{
    macros::{map, tracepoint},
    maps::HashMap,
    programs::TracePointContext,
};
use aya_log_ebpf::info;

/// Key: destination IP + port + sk_bound_dev_if (interface)
#[repr(C)]
pub struct RetransmitKey {
    pub daddr: u32,      // destination IPv4 address
    pub dport: u16,      // destination port
    pub bound_dev: u16,  // interface index (0 = any)
}

/// Value: count of retransmits in this reporting window
#[repr(C)]
pub struct RetransmitValue {
    pub count: u64,
    pub last_ts: u64, // last retransmit timestamp (ns)
}

/// Map: per-destination retransmit counts
/// Userspace reads and clears this map every reporting interval.
#[map]
static RETRANSMITS: HashMap<RetransmitKey, RetransmitValue> = HashMap::with_max_entries(1024, 0);

/// Total retransmit counter (single entry) for quick "is anything happening?" check
#[map]
static TOTAL_RETRANSMITS: HashMap<u32, u64> = HashMap::with_max_entries(1, 0);

/// Tracepoint: tcp/tcp_retransmit_skb
///
/// Format from /sys/kernel/tracing/events/tcp/tcp_retransmit_skb/format:
///   field:const void * skbaddr;  offset:8;  size:8;
///   field:const void * skaddr;   offset:16; size:8;
///   field:int state;             offset:24; size:4;
///   field:__u16 sport;           offset:28; size:2;
///   field:__u16 dport;           offset:30; size:2;
///   field:__u16 family;          offset:32; size:2;
///   field:__u8 saddr[4];         offset:34; size:4;
///   field:__u8 daddr[4];         offset:38; size:4;
///   field:__u8 saddr_v6[16];     offset:42; size:16;
///   field:__u8 daddr_v6[16];     offset:58; size:16;
#[tracepoint]
pub fn tcp_retransmit(ctx: TracePointContext) -> u32 {
    match try_tcp_retransmit(&ctx) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn try_tcp_retransmit(ctx: &TracePointContext) -> Result<(), i64> {
    // Read fields from tracepoint context
    // Offsets are from the format file above
    let dport: u16 = unsafe { ctx.read_at(30)? };
    let family: u16 = unsafe { ctx.read_at(32)? };

    // Only handle IPv4 for now (family == 2)
    if family != 2 {
        return Ok(());
    }

    // Read destination IPv4 address (4 bytes at offset 38)
    let daddr: u32 = unsafe { ctx.read_at(38)? };

    let key = RetransmitKey {
        daddr,
        dport: u16::from_be(dport),
        bound_dev: 0,
    };

    // Increment per-destination counter
    if let Some(val) = unsafe { RETRANSMITS.get_ptr_mut(&key) } {
        unsafe {
            (*val).count += 1;
            (*val).last_ts = aya_ebpf::helpers::bpf_ktime_get_ns();
        }
    } else {
        let val = RetransmitValue {
            count: 1,
            last_ts: unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() },
        };
        RETRANSMITS.insert(&key, &val, 0).ok();
    }

    // Increment total counter
    let zero: u32 = 0;
    if let Some(total) = unsafe { TOTAL_RETRANSMITS.get_ptr_mut(&zero) } {
        unsafe { *total += 1; }
    } else {
        TOTAL_RETRANSMITS.insert(&zero, &1u64, 0).ok();
    }

    Ok(())
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
