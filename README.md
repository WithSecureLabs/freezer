# Freezer

Freezer is a tool to download resources from a Kubernetes cluster in the same format needed for [IceKube](https://github.com/WithSecureLabs/IceKube). Specifically, it is a rust implementation of the `icekube download` command.

## Usage

```bash
# Quick run and download to dir/
cargo run dir/

# Build and run, the binary can also be uploaded elsewhere to be run from another location
cargo build --release
./target/release/freezer dir/
```


## Permissions Required

This requires elevated privileges within the target cluster to enumerate resources. This typically requires read-only access on all resources within the cluster including secrets. Freezer does not persist any secret data it retrieves from secrets if that is a concern. 
