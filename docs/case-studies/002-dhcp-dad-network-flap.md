# Case Study: Host Network Flapping — DHCP DAD Conflict

**Host:** astra (NixOS, 8-core, 64GB RAM)
**Time to root cause:** 30 seconds
**Symptom:** Intermittent 502 errors on all hosted services

## The incident

All services behind the Cloudflare tunnel (Gitea, web apps, APIs) intermittently returned 502 Bad Gateway. The K8s cluster appeared healthy — all pods Running, no CrashLoopBackOff. The issue was invisible to cluster-level monitoring.

## What Savants found

`savants host snapshot` + `savants story` revealed:

1. **361 DHCP DAD (Duplicate Address Detection) failures** on the WiFi interface `wlp170s0`
2. Device `5e:a6:e6:0c:06:a8` (randomized MAC — a phone or laptop) was claiming IP `192.168.100.243`
3. The router kept offering the same conflicting IP to astra
4. Every DAD cycle (every ~6 seconds) interrupted the host's network connectivity
5. Cloudflared logged "Unable to reach the origin service" during each interruption

## Why cluster monitoring missed it

K8s pod health checks are internal (pod-to-pod via the CNI network). The host's WiFi flapping didn't affect internal cluster networking — only external connectivity through the Cloudflare tunnel. Traditional K8s monitoring (pod status, liveness probes) showed everything green.

**Only the host layer caught it.** The journal error `dhcpcd: wlp170s0: DAD detected 192.168.100.243` appeared 361 times but was invisible to any pod-level tool.

## The fix

Switched from DHCP to a static IP in the NixOS configuration:

```nix
networking.interfaces.wlp170s0.ipv4.addresses = [{
  address = "192.168.100.148";
  prefixLength = 24;
}];
networking.defaultGateway = "192.168.100.1";
networking.nameservers = [ "1.1.1.1" "8.8.8.8" ];
```

Applied with `nixos-rebuild switch`. Zero downtime — connectivity verified within seconds.

## Safety approach

Since astra is headless (no display), we:
1. Verified the target IP (`.148`) was free via ping
2. Started a background revert script that would restore DHCP after 5 minutes if connectivity failed
3. Applied the change
4. Verified connectivity
5. Cancelled the revert

## Key metrics

| Metric | Value |
|---|---|
| DAD failures | 361 occurrences |
| Cloudflared connection drops | 44 events |
| 502 errors served to users | Unknown (intermittent) |
| Time to diagnosis | 30 seconds |
| Time to fix | 3 minutes (including safety net) |
| Layer that caught it | Host (journal errors) |
| Layers that missed it | K8s (all pods Running) |

## Why this matters

This incident proves why the host monitoring layer exists. A cluster-only tool would have said "everything is healthy" while users saw 502 errors. The cross-layer architecture — host + cluster + logs in one graph — is what caught the real root cause: a DHCP conflict on the physical network interface that no pod-level monitoring could detect.
