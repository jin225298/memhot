# Pi web plugin static transform cache

Each web tool call still launches a new Node process and uses a new private temporary directory. Only Jiti's compiled plugin source is reused. Queries, webpages, responses, credentials, configuration and plugin runtime state are never part of the release cache.

## Release preparation

Run the pinned Node binary with an empty environment, as root on Linux, after `npm ci` at the final absolute release path and before SDK tests and the dependency digest:

```sh
/opt/memhot/runtime/node-v22.23.3-linux-x64/bin/node --max-old-space-size=128 /opt/memhot/releases/RELEASE/agent/src/seed-web-cache.mjs /opt/memhot/releases/RELEASE/agent
```

The helper imports the actual pinned `pi-web-access` 0.38.0 standalone core with Jiti 2.7.0. Fixed in-memory HTTP fixtures and mocked DNS exercise search and readable extraction without network or model calls. The helper deletes its temporary configuration. Run it in the release preparation sandbox with network denied. It rejects an existing template; `npm ci` removes previous derived files before regeneration.

The resulting `agent/node_modules/.cache/memhot-web-jiti-v1` is root-owned, directories 0755 and files 0644. Its manifest binds the final SDK path, exact Node version, package lock, plugin TypeScript inventory, Jiti compiler source, dependency versions and every compiled file's SHA256. Include all generated files in the existing dependency digest. The seed helper and runtime module are part of the release source manifest. A cache copied from another installation path is rejected because Jiti's filenames also depend on the source's absolute path.

## Runtime boundary

Before using a template, the child checks ownership, permissions, absence of symlinks, exact inventory and all manifest hashes. It then writes a fresh copy into its own 0700 sandbox with files 0600. Jiti independently checks its source-content hash on each cache hit. A lazy transform can only modify that child's disposable copy. Completion, timeout and host termination remove the child directory; the release tree is never written by the service.

Missing, stale, altered, oversized or untrusted templates fall back to `fsCache:false` and cold compilation. They cannot load partially verified cached code. Jiti disables a directly read-only `fsCache` directory, so the private writable copy is necessary.

The Rust child deadline is 60 seconds, including the serialized tool queue. Search allows up to two providers within an overall 110 seconds. The Node web worker's overall/RPC deadline is 60 seconds. Public HTTP validation, response limits and redirect rules are unchanged. These limits allow cold compilation when the template is absent; a valid cache normally needs only a few seconds under the measured CPU quota.

## Measurements

Sequential offline measurements on the release Linux host, Node 22.23.3, 384 MiB memory cap, real plugin source and fixed fixture responses:

| Mode | CPU quota | First HTTP | Total | Peak RSS |
| --- | --- | --- | --- | --- |
| No transform cache | 50% | 24.898 s | 35.784 s | — |
| Immutable template copied privately | 50% | 1.177 s | 3.662 s | 81.1 MiB |
| Immutable template copied privately | 100% | 0.685 s | 2.416 s | 81.3 MiB |

The template contains 72 compiled files (879,897 bytes before its manifest). Copying took 84.7 ms at 50% and 44.5 ms at 100%; no OOM events occurred. These initial figures measure copying before the additional runtime manifest validation. Release acceptance also runs the actual worker concurrently with a native Pi session after seeding, with all network/model traffic replaced by fixtures. No paid calls are needed.

Focused tests cover independent child copies, no shared runtime state, stale source/lock/install bindings, missing cache, modified compiled bytes, unexpected files, traversal, unsafe permissions, nested source symlinks, and unrecognized compiler versions or format.


Final installed runtime verification (same Linux release, CPU 100%, 384 MiB cap) includes all manifest/source hashing and the private copy. With the SDK test suite also using the host, the native Pi + actual web worker smoke completed in 24.292 s: SDK first frame 12.842 s, web first HTTP 3.181 s; combined cgroup peak 145,444,864 bytes, no high/max/OOM events. A separate actual public-network check returned 10 Degoog results in 3.489 s and extracted 3,572 characters from the official SQLite page in 2.522 s, with peak 54,898,688 bytes and zero model calls. These are measured checks, not latency guarantees.
