# Immutable legacy source pointers

The contents API does not support an atomic server-side move, so the first refactor commit quarantines the legacy implementation by immutable Git blob pointer while the replacement compiles alongside it. These blobs are the exact pre-cutover sources and can be recovered without relying on the active paths.

| Legacy source | Git blob SHA | Status |
|---|---|---|
| `src/r2_attach.rs` | `e20ce4891d6198ba304fca97cf14c63630e46345` | old monolithic R2 orchestration; pending removal from active graph |
| `crates/bumble-adapter/src/pairing.rs` | `215e49ffb768177a4883c6f91175716e6a68a321` | manual SMP + persistence experiment |
| `crates/bumble-adapter/src/connect.rs` | `b42d295e4f03a7b35d66af62c2115c3b1d0c09b6` | explicit-address/local-RPA connection experiments |
| `crates/bumble-adapter/src/bond.rs` | `59a589f728d8d2860270f756347617037c0dc29b` | mixed peer store + local identity reseed experiment |

Recovery example:

```text
git cat-file -p <blob-sha>
```

The next cutover phase will replace active references with narrow adapters and then delete the superseded files or rewrite them to contain only reusable primitives. Git history and these blob IDs preserve the original implementation.
