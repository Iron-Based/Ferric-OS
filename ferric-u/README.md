# Ferric-U

The userland for ferric.

## libc

`libc/` is `libferric`, the freestanding Zig C library the kernel links statically. Its toolchain
contract (Zig 0.16 flags, `compiler_rt` policy, no-`std` gate) is documented in
[`libc/README.md`](libc/README.md#toolchain-contract).
