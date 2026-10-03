# ferric-u/libc

`libferric` — the freestanding ISO C library for Ferric-OS, written in Zig. It produces
`libferric.a` per architecture (`x86_64`, `aarch64`), which the kernel links statically so that
Zig owns `mem*`/string/`printf`/`malloc` and Rust owns only the syscall layer.

**Status:** the toolchain contract below is settled and measured; no sources are built yet
(`build.zig` and `src/root.zig` are still to come).

## Toolchain contract

Everything below was measured against **Zig 0.16.0** (`zig cc --version` → `clang version 21.1.0`)
on both `*-freestanding-none` targets. Zig 0.16.0 is the pin; a different `zig version`
invalidates every claim here, so treat a version bump as a re-verification of this file.

### Canonical invocations

| Arch | Zig target | Flags |
|---|---|---|
| x86_64 | `x86_64-freestanding-none` | `-OReleaseSmall -fno-unwind-tables -mno-red-zone -mcmodel=kernel -mcpu=x86_64-mmx -fno-PIC -fno-stack-check` |
| aarch64 | `aarch64-freestanding-none` | `-OReleaseSmall -fno-unwind-tables -mcmodel=small -mcpu=generic+strict_align -fno-PIC -fno-stack-check` |

```sh
# x86_64
zig build-lib -target x86_64-freestanding-none \
  -OReleaseSmall -fno-unwind-tables -mno-red-zone \
  -mcmodel=kernel -mcpu=x86_64-mmx -fno-PIC -fno-stack-check \
  --name ferric -femit-bin=ferric-u/build/x86_64/libferric.a src/root.zig

# aarch64
zig build-lib -target aarch64-freestanding-none \
  -OReleaseSmall -fno-unwind-tables \
  -mcmodel=small -mcpu=generic+strict_align -fno-PIC -fno-stack-check \
  --name ferric -femit-bin=ferric-u/build/aarch64/libferric.a src/root.zig
```

`-femit-bin` with a `.a` extension emits a real `ar` archive (`!<arch>` magic, symbol index
included), which is what the kernel links and what the `e_machine` gate reads member-by-member.
`zig build-obj` with the identical flags (minus `--name`) is the cheap research form.

### Why each flag

| Flag | Reason |
|---|---|
| `-target <arch>-freestanding-none` | No OS, no libc, no start files. Mirrors the `*-unknown-none` LLVM triple in `ferric-k/targets/{x86_64,aarch64}-ferric.json`, which is what Rust compiles the kernel against. |
| `-OReleaseSmall` | Freestanding release mode is mandatory, not a size preference. A single `@panic` in a Debug build drags in the `debug.FullPanic` machinery — a 126,016-byte object (message spilling plus a `ud2` trap), still with zero undefined symbols, so nothing fails to link and the bloat is silent. `-OReleaseSafe` is also self-contained (panic → inline `ud2`, 27,232-byte object) but keeps overflow checks we do not want in a libc. |
| `-fno-unwind-tables` | Default output emits `.eh_frame` + `.rela.eh_frame`. `ferric-k/kernels/*.ld` `/DISCARD/`s `.eh_frame` but not `.rela.eh_frame`, so relying on the linker script leaves a relocation section behind. The flag removes both. |
| `-mno-red-zone` (x86_64 only) | Rust's target spec sets `disable-redzone`. Without the flag LLVM uses `[rsp-N]` scratch space for leaf frames under `-fomit-frame-pointer`; with it, a probe leaf emits `sub rsp, 64`. Accepted but a no-op on aarch64, so it is x86-only here. |
| `-mcmodel=kernel` / `-mcmodel=small` | x86_64 `kernel` is accepted and behaves like the default for static data (`R_X86_64_32S`); it is pinned for explicitness. aarch64 **must** be `small`: `kernel` aborts the compiler with `LLVM ERROR: Only small, tiny and large code models are allowed on AArch64`. |
| `-mcpu=x86_64-mmx` / `-mcpu=generic+strict_align` | Feature parity with the Rust specs. x86_64 resolves to `+mmx +sse +sse2 +x87` (Rust asks for `-mmx,-sse,+sse2`, and SSE2 implies SSE). aarch64 Zig's default CPU is `-strict-align`; `generic+strict_align` flips it to `+strict-align` with NEON on. Zig CPU feature names use `_`, never `-`: `strict-align` fails as an unknown feature. |
| `-fno-PIC` | The relocation model is static. Confirmed relocation kinds: x86_64 `R_X86_64_32S` / `R_X86_64_PLT32` / `R_X86_64_PC32`; aarch64 `R_AARCH64_ADR_PREL_PG_HI21` + `R_AARCH64_ADD_ABS_LO12_NC`, `CALL26`/`JUMP26`. No GOT relocations on either arch. |
| `-fno-stack-check` | Already the default; passed explicitly so it cannot regress. `-fstack-check` would introduce `__zig_probe_stack`, which nothing in a kernel provides. `-fstack-protector` is unavailable: `enabling stack protection requires libc`. |

### Page size and sections

Emitted sections are `.text`, `.rodata`, `.rodata.cst16`, `.data`, `.bss`, `.note.GNU-stack`, with
a maximum alignment of 16 bytes — comfortably inside the 4 KiB contract. The page size itself is
already enforced at link time by `.cargo/config.toml` (`-C link-arg=-z
-C link-arg=max-page-size=0x1000`) for both kernel targets, so no Zig-side max-page flag is needed.
`.note.GNU-stack` is covered by the scripts' `/DISCARD/ .note*`.

### ABI and symbol naming

- `export fn` is already the C ABI. In Zig 0.16 the explicit spelling is **`callconv(.c)`** —
  lowercase; `callconv(.C)` is the older spelling and fails with ``union 'builtin.CallingConvention'
  has no member named 'C'``.
- `export fn` symbols land with their plain names and no leading underscore (`T memcpy`,
  `T __udivti3`); `pub` is irrelevant for exported symbols.
- Rust consumes them through `extern "C"` declarations plus
  `#[link(name = "ferric", kind = "static")]`. Both `llvm-nm` and GNU `nm` read the emitted archive.

### `compiler_rt`: hand-written shim, not `-fcompiler-rt`

**Do not pass `-fcompiler-rt`.**

It works, but it injects compiler-rt at whole-object granularity: ~301 KB of extra payload
carrying weak definitions of `memcpy`, `memmove`, `memset`, `memcmp`, `strlen`, `bcmp` and a pile
of libm entry points. Those weak `mem*` definitions are exactly the symbols `libferric` must own —
`ferric-unsafe-core/src/mem.rs` is removed once the archive is linked into the kernel — so the
payload is both bloat and a duplicate-ownership hazard.

Instead, Zig leaves 128-bit built-ins as ordinary undefined calls, and we implement exactly those.
Measured on a probe using `u128` `/`, `%`, `<<` by a runtime amount, `u64`↔`f32`/`f64`
conversions, `f64` arithmetic and byte/word loops, the undefined set on **both** arches is:

```
__ashlti3  __udivti3  __umodti3  memcpy  memmove  memset  strlen
```

Reading of that list:

- `__udivti3` ← `u128` `/`; `__umodti3` ← `u128` `%` (LLVM does **not** reuse the quotient helper
  for the remainder, so both are required); `__ashlti3` ← `<<` by a non-constant amount.
- No float helper appeared. The tested int↔float conversions and `f64` arithmetic are all inlined
  by the backend; re-check the allowlist when soft-float or `f128` code first appears.
- `memcpy` / `memmove` / `memset` / `strlen` are Zig's own lowering (struct copies and the
  `memset`-style idiom recognizer turning loops into calls). These are symbols `libferric`
  **defines**, not externals it needs.

The shim strategy is verified: a Zig-defined `pub export fn __udivti3(...) callconv(.c)` becomes a
strong `T __udivti3` in both arches' objects, after which the only remaining undefined symbol is
`__umodti3`. So the shim starts from `__ashlti3`, `__udivti3`, `__umodti3` (adding `__udivmodti4` if
a future lowering asks for it), and the xtask `nm` allowlist stays exactly: **syscall trampoline +
this shim**. Nothing else may remain undefined.

### No `std` is a gate, not a compiler guarantee

`-target ...-freestanding-none` does **not** make the standard library unreachable:
`@import("std")` compiles fine and produces a 112,960-byte object with `builtin`/`Target` data
inlined. Enforcement is therefore a source gate in `cargo xtask check`, over
`ferric-u/libc/src/**.zig`:

- reject `@import("std")` and bare `std.` references in shipped libc sources;
- enforce the `nm` undefined-symbol allowlist above, which catches std creeping in as a link
  dependency.

Host tests (`zig build test`, native target) are explicitly allowed to use std — only the
freestanding archive is held to the rule.

### `zig cc`

Available and usable for the header work (real `.h` headers verified with `zig cc`), with traps
worth knowing before the headers land. The header gate is `zig cc -ffreestanding -target <arch>`
per header; the stricter variant below additionally removes the bundled libc headers:

```sh
zig cc -target x86_64-freestanding-none -ffreestanding -fno-builtin -nostdinc \
  -Iferric-u/libc/include -x c -c someheader.h -o /tmp/someheader.o
```

- Use `-c`; `-fsyntax-only` misbehaves here (`FileNotFound`).
- `-nostdinc` drops the bundled libc headers (`stdio.h` stops resolving) but compiler-provided
  headers such as `stddef.h` still do, so a header can pass self-containment while leaning on a
  compiler builtin header. `-nobuiltininc` is *not* honored (`argument unused during compilation`).
  If builtin-header independence matters, assert on the resulting object/dependencies instead.

## API coverage matrix

Populated as implementations land: the `<stddef.h>` family, then `<string.h>`/`<strings.h>`, then
`<stdlib.h>` numerics, then `stdio`/`printf`, time/env, and the file API. The authoritative
per-function state is the xtask `nm` undefined-symbol allowlist plus the `cargo xtask check` gates,
not this table.

## Unsupported by design

Out of scope on purpose: sockets/networking, threads and `futex`, `epoll`/`select`, writable
filesystems, `ptrace`, `seccomp`, dynamic linking (`dlopen`), and SMP (single-CPU by construction
today). Per-function omissions are tracked in the matrix above; nothing is silently unimplemented.

## Reproducing these results

`probe.zig`:

```zig
export fn probe(x: u128) callconv(.c) u128 {
    return (x / 7) % (x % 7);
}
```

```sh
zig build-obj -target x86_64-freestanding-none -OReleaseSmall -fno-unwind-tables \
  -mno-red-zone -mcmodel=kernel -mcpu=x86_64-mmx -fno-PIC -fno-stack-check \
  -femit-bin=probe.o probe.zig
nm --undefined-only probe.o        # llvm-nm on Windows toolchains
llvm-objdump -h probe.o
```

Both arches emit `U __udivti3` / `U __umodti3` from that one line, and the object carries only
`.text`, `.rela.text`, `.note.GNU-stack` and the symbol tables — no `.eh_frame`, no `.rodata`
padding. The wider undefined set quoted above (`__ashlti3`, `memcpy`, `memmove`, `memset`,
`strlen`) comes from a probe that also does `u128 << runtime`, `u64`↔`f32`/`f64` conversions and
byte/word loops; add those operations and re-run to grow the list.