# Resource Adapter System

## Overview

Resource adapters are declarative TOML contracts that define how Savants
ingests any resource type — K8s built-ins, CRDs, host data, cloud
resources. The binary loads adapters at runtime. New resource types =
new TOML file, not new code.

## Architecture

```
~/.savants/adapters/
    ├── k8s-core-v1.toml        # Pods, Services, ConfigMaps, Secrets
    ├── k8s-apps-v1.toml        # Deployments, StatefulSets, DaemonSets
    ├── host-linux-v1.toml      # /proc, systemd, dmesg
    ├── aws-ec2-v1.toml         # EC2 instances
    └── custom-my-crd-v1.toml   # Customer CRDs

savants-cli binary
    ├── adapter_loader.rs       # Parses TOML, validates schema
    ├── adapter_engine.rs       # Executes adapters: fetch → map → write
    └── query_generator.rs      # Generates queries from adapter definitions
                                # (compiled Rust, never exposed)
```

## Adapter TOML format

```toml
[adapter]
name = "k8s-core"
version = "1.0.0"
min_savants_version = "0.1.0"
source = "k8s-api"  # k8s-api | procfs | command | http

[[resources]]
name = "Pod"
label = "K8sPod"

# For K8s resources:
api_group = ""
api_version = "v1"
kind = "Pod"
list_endpoint = "/api/v1/pods"
namespaced = true

# For host resources:
# source_type = "procfs"
# source_path = "/proc/meminfo"
# parser = "key_value"  # key_value | json | regex | table

[resources.key]
fields = ["name", "namespace", "cluster"]

[resources.properties]
name = "metadata.name"
namespace = "metadata.namespace"
status = "status.phase"
# JSONPath expressions map API response → graph properties

[[resources.edges]]
type = "CONTAINS"
from_label = "K8sNamespace"
from_match = { name = "$.namespace", cluster = "$.cluster" }
```

## IP protection

- Adapter TOMLs contain declarative mappings (public, customer-editable)
- Query generation logic is compiled into the Rust binary (private)
- The binary translates "field X → property Y" into optimized queries
- A customer reading the TOML knows WHAT data is collected
- They do NOT know HOW it's stored or queried internally

## Auto-update

```bash
savants adapter update
# Checks savants.dev/adapters/manifest.json
# Downloads new/updated adapters
# No binary update needed
```

## Customer extensibility

```bash
savants adapter create my-crd \
  --api-group mycompany.io \
  --api-version v1 \
  --kind MyCustomResource
# Generates template TOML, customer fills in field mappings
```

## Versioning

- Adapters are versioned independently from the binary
- min_savants_version ensures compatibility
- Breaking changes = new major version (v1 → v2)
- Old adapters continue to work with new binaries
- New adapters may not work with old binaries (min version check)
