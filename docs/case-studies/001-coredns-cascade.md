# Case Study: 15 CrashLoopBackOff Pods — One DNS Config Change

**Cluster:** astra-k3s (94 pods, 33 namespaces)
**Time to root cause:** 45 seconds
**Without Savants:** Estimated 45-60 minutes

## The incident

15 pods across 8 namespaces entered CrashLoopBackOff simultaneously. The cluster appeared to be in a cascade failure with no obvious single cause.

## What Savants found

`savants up` ingested 7.5 million log lines from 94 pods and compressed them to 78 distinct error templates. The diagnosis:

1. **CoreDNS** was forwarding to Tailscale MagicDNS (`100.100.100.100`) which was intermittently unreachable
2. DNS failures cascaded to every pod that needed to resolve `*.svc.cluster.local`
3. Leader election for cert-manager, crossplane, and other controllers failed because they couldn't reach the API server
4. Applications (Temporal, Immich, Vikunja, Authentik) couldn't resolve their database hostnames
5. Cloudflare tunnels dropped because they couldn't resolve Cloudflare edge POPs

## The cross-layer connection

Savants connected the log errors across pods to a single ConfigMap:

```
LogEvent (coredns: "read udp → 100.100.100.100: i/o timeout")
  → MENTIONS → K8sConfigMap (coredns)
    → READS ← K8sPod (temporal-worker, immich-server, cert-manager-cainjector, ...)
```

Every crashing pod's error traced back to the same DNS failure, which traced back to one ConfigMap.

## The fix

One command:
```bash
kubectl -n kube-system edit configmap coredns
# Changed: forward . /etc/resolv.conf
# To:      forward . 1.1.1.1 8.8.8.8
```

All 15 pods self-healed within 5 minutes.

## Key metrics

| Metric | Value |
|---|---|
| Pods affected | 15 (CrashLoopBackOff) |
| Namespaces affected | 8 |
| Cumulative restarts | 365 (cert-manager-cainjector alone) |
| Log lines analyzed | 7,500,000 |
| Error templates found | 78 |
| Root cause templates | 1 |
| Time to diagnosis | 45 seconds |
| Time to fix | 2 minutes |
