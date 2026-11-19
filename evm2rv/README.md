# evm2rv

`evm2rv` translates EVM runtime bytecode (what `solc --bin-runtime` prints) into C over
`evm-rt/`. `rand-guest build` then compiles that C into a zkVM image. The image takes the same
input vector as the EVM interpreter guest (`guests-compiled/bin/evm.bin`) and publishes the same
eight output words. It just runs the contract natively instead of interpreting it opcode by
opcode.

The design spec is `docs/superpowers/specs/2026-09-18-evm-to-rv32-translator-design.md`. Every
ruling is in the task briefs and reports under `.superpowers/sdd/2026-09-18-evm2rv-translator/`.

There are two translation stages. Both give the same results and the same gas.

| stage | source | what it does | flag |
|---|---|---|---|
| one | `src/emit.rs` | every opcode over `evm-rt`'s memory stack | `--stage 1` |
| two | `src/lift.rs` | each block's words in C locals, constants folded at translation | `--stage 2` (the default) |

Each stage is a different program, so each has its own `hc`.

## Walkthrough: the ERC-20, end to end

Every command below was run from the root of a `circuits` checkout, and the output shown is its
real output. Long output is cut where marked `...`.

### 0. The toolchain

The pins are Rust 1.98.1, clang 23.1.1 and the `cc` crate 1.4.6. `hc` depends on all three.

```
$ /opt/homebrew/opt/llvm/bin/clang --version | head -1
Homebrew clang version 23.1.1
```

On macOS, `brew install llvm` gives this clang. Build the two tools once:

```
$ (cd evm2rv && cargo +1.98.1 build --release)
$ (cd rand-guest && cargo +1.98.1 build --release)
```

### 1. The bytecode

`evm2rv` takes **runtime** bytecode: the code that lives at the contract's address after
deployment. It does not take creation code. Creation code runs the constructor once and returns
the runtime code. A proof here is of one call to a deployed contract, so the constructor never
runs. The state it would have written is part of the pre-state instead, as storage witnesses.

To get runtime bytecode from Solidity:

```
solc --optimize --optimize-runs 200 --evm-version shanghai --bin-runtime MyToken.sol
```

The last line of output is the hex. Save it as `mytoken.hex`. `--evm-version` may be `shanghai`,
`cancun`, `prague` or `osaka`: `MCOPY`, `TLOAD`/`TSTORE` (transient storage, at most 16 non-zero
slots per call, `MAX_TRANSIENT_SLOTS`; a 17th halts `OutOfBounds`) and `CLZ` translate. `BLOBHASH` and `BLOBBASEFEE` trap and are warned.

`solc` is not installed on the machine these numbers come from. The committed ERC-20
(`guests-compiled/evm/contracts/erc20.runtime.hex`, 1 296 bytes) was compiled once with solc
**0.8.37** (`0.8.37+commit.f401782d`), with exactly the flags above.
`guests-compiled/evm/contracts/SOLC.md` names the binary and its sha256.
`guests-compiled/evm/contracts/build.sh` recompiles it and diffs the result when a solc is on
`PATH`.

### 2. Translate

```
$ evm2rv/target/release/evm2rv guests-compiled/evm/contracts/erc20.runtime.hex --out evm2rv/target/erc20 --chain-id 12
1296 code bytes: 74 blocks, 798 opcodes, 51 jumpdests (stage 2)
CHAINID is the constant 12
no trapping opcodes present
wrote evm2rv/target/erc20 (crate erc20-runtime): contract.c, Cargo.toml, Cargo.lock, build.rs, src/main.rs, shim.ld
```

- A `.hex` file is read as hex text. Any other extension is read as raw bytes.
- `--out` must be inside a circuits checkout, because `rand-guest build` needs one.
- `--chain-id N` is the value `CHAINID` returns, baked into the C. It is required when the code
  contains `CHAINID`. This ERC-20 does not, so the flag changes nothing here: `hc` is the same with
  or without it.
- Any trapping opcode is listed as a warning, with its pc.

`contract.c` is the translated contract. It ends with the code guard (see "Trust" below):

```
$ tail -7 evm2rv/target/erc20/contract.c
   from. Before any of the code above runs, the shim hashes the input vector's code the same way and
   refuses any other (OutOfBounds, status 2, gas_used 0). */
const uint32_t *evm_code_digest(void);
const uint32_t *evm_code_digest(void) {
    static const uint32_t d[8] = {0xf679280fu, 0x86c10e83u, 0x7e8f1f39u, 0xa2377de3u, 0x8a2e282fu, 0xc54cad42u, 0xddfaf009u, 0x7d28a8afu};
    return d;
}
```

### 3. Build the image

```
$ rand-guest/target/release/rand-guest build evm2rv/target/erc20 --max-words 65535 --allow-panic
...
warning: erc20-runtime@0.1.0: evm2rv: clang Homebrew clang version 23.1.1 (/opt/homebrew/opt/llvm/bin/clang)
    Finished `release` profile [optimized] target(s) in 3.50s
7965 words against a cap of 65535 (fits); 0 ecall(s) with a non-static a7
OK
wrote evm2rv/target/erc20/image.bin and its .sha256 (7965 words, hc fd8dd3afdd785a991993c0f17dcca9a7c2c4b4c3bf213e2354d9aa1ffbf6e400, program id 5591baf0b5f918a40e89dcb6f2b31fec1443dd0b6cb0059a7f9753e672d550a8)
```

`--max-words 65535` is needed: the default cap is 4 096 words. `--allow-panic` is needed too: the shim links
`core`'s panic machinery through the ABI harness, which `rand-guest build` otherwise rejects (a panic
halts, and a halted run proves — `rand-guest/README.md`, "Rejecting"; a consumer must treat
`out[7] == 0xdeadbeef` as a rejection). The build refuses any clang other
than 23.1.1. `RAND_GUEST_CLANG_UNPINNED=1` builds anyway, with a warning that `hc` will not match
published images.

### 4. Run a transfer

The input vector is the interpreter's: the code, the calldata, the caller and other environment
words, the gas limit, the pre-state root, and one storage witness per slot the call touches. This
example prints the parity test's `transfer` (ALICE sends BOB 250 of her 1 000) as 921 words. It
also has `approve` and `transferFrom`.

```
$ (cd evm2rv && cargo +1.98.1 run -q --release --example erc20_vector -- transfer > target/transfer.words)
1441 input words
$ rand-guest/target/release/rand-guest run evm2rv/target/erc20/image.bin --input $(cat evm2rv/target/transfer.words)
out[0] = 1
out[1] = 2053220150
out[2] = 2668629896
out[3] = 4244819158
out[4] = 80492969
out[5] = 1294998629
out[6] = 476639945
out[7] = 2694991971
cycles 59946
tier 16
```

`out[0]` is the status: 1 success, 0 revert, 2 exceptional halt. `out[1..8]` is the `EVM_OUT`
digest over the storage layout, the code hash, both state roots, the return data, the logs (topics and data), the
call (address, caller, value, calldata, gas limit, context) and the gas used
(`research/docs/04-guests.md`).

### 5. The same eight words as the interpreter

The interpreter guest (`evm.bin`, storage layout 2, program id `0eff0627…`, hc `3869b014…` since set 11's block context, `7862f530…` before it, `fae9d774…` before set 11), on the
same input:

```
$ rand-guest/target/release/rand-guest run guests-compiled/bin/evm.bin --input $(cat evm2rv/target/transfer.words)
out[0] = 1
out[1] = 2053220150
out[2] = 2668629896
out[3] = 4244819158
out[4] = 80492969
out[5] = 1294998629
out[6] = 476639945
out[7] = 2694991971
cycles 121737
tier 18
```

The interpreter run natively on the host (the tests' oracle):

```
$ (cd evm2rv && cargo +1.98.1 run -q --release --example erc20_vector -- transfer --expected)
out[0] = 1
out[1] = 2053220150
...
out[7] = 2694991971
interpreter (native): status 1, Return, gas_used 29956
```

The check, as one command:

```
$ diff <(rand-guest/target/release/rand-guest run evm2rv/target/erc20/image.bin --input $(cat evm2rv/target/transfer.words) | grep '^out') \
       <(rand-guest/target/release/rand-guest run guests-compiled/bin/evm.bin --input $(cat evm2rv/target/transfer.words) | grep '^out') \
  && echo "the eight words are identical"
the eight words are identical
```

| | translated (stage two) | interpreter (`evm.bin`) |
|---|---:|---:|
| eight output words | identical | identical |
| cycles | 59 946 | 121 737 |
| tier | 16 | 18 |
| program words | 7 965 | 15 531 |

`tests/parity.rs` runs this and seven more vectors under both stages, and also checks `gas_used`
and the halt through a second build that reports them.

### 6. Deploy (not run here)

```
$ rand-guest/target/release/rand-guest info evm2rv/target/erc20/image.bin
form: image container
text …; data …; prologue … words; program 7965 words from base_pc …
hc fd8dd3afdd785a991993c0f17dcca9a7c2c4b4c3bf213e2354d9aa1ffbf6e400
program id 5591baf0b5f918a40e89dcb6f2b31fec1443dd0b6cb0059a7f9753e672d550a8
7965 words against a cap of 4096: does not fit
```

The fullnode wallet deploys an image with:

```
rand program deploy evm2rv/target/erc20/image.bin
```

It prints the program id, the word count and `hc`, then checks the chain's program cap before it
proves anything. The image is 7 965 words. Chain 12 runs the default cap of 4 096 words, so this
deploy is refused there. It needs a chain whose genesis sets `max_program_words` high enough, for
example `rand-node genesis ... --max-program-words 65535` (fullnode `docs/cli.md`). The cap is part
of the genesis hash, so it cannot be raised on an existing chain.

## Trust

A verifier checks a translated contract in two steps:

1. **`hc == translate(bytecode)`.** Re-run `evm2rv` on the published bytecode with the same
   `--stage` and `--chain-id`, rebuild with the pinned toolchain (Rust 1.98.1, clang 23.1.1,
   `cc` 1.4.6), and compare `hc`. The generated crate pins `cc` exactly and carries its own
   `Cargo.lock`. The build refuses any other clang unless told otherwise.
2. **The code guard.** The translated logic is baked into the image, so `hc` binds it. But
   `CODECOPY` and `CODESIZE` read the input vector's code. Without a check, one `hc` could run with
   code bytes the caller chose. So `contract.c` carries a digest of the source bytecode
   (`guard::code_digest`: the `POSEIDON2` sponge over the code's length and its bytes), and the
   shim hashes the input vector's code the same way before any translated code runs. Any other
   code is refused the way the interpreter's `pre_halt` stops a call before it starts: status 2,
   `gas_used` 0, halt `OutOfBounds`. So `hc` binds the code too.

The `EVM_OUT` digest also binds `keccak256` of the input vector's code, as the interpreter's
does. The chain does not record the source bytecode or its hash; a verifier who wants to check
`hc` needs the bytecode from the contract's author.

What the guard costs, on the transfer (measured when the guard was added, before ZKVM-103/104
moved every figure in this file; the difference is the guard's):

| | before the guard | with the guard | cost |
|---|---:|---:|---:|
| stage one cycles | 79 203 | 80 211 | +1 008 |
| stage two cycles | 65 227 | 66 235 | +1 008 |
| program words, either stage | 15 637 / 11 519 | 15 804 / 11 686 | +167 |

Why `POSEIDON2` and not Keccak: it is the cheaper coprocessor hash (the table below, measured when
the guard was added). Since ZKVM-117 it is also the *only* hash of the code the guest computes:
`EVM_OUT` carries `keccak256(code)`, the translator bakes that into the image beside the guard's
digest, and the shim hands it to the harness once the guard has passed, so the harness does not
hash the same bytes a second time with Keccak (ten permutations for the ERC-20). When the guard
refuses the code the harness hashes the code it was given, as before.

| guard hash (stage two transfer) | added cycles |
|---|---:|
| Keccak-256 of the code | +8 798 |
| `POSEIDON2`, plain copy loop | +2 720 |
| `POSEIDON2`, eight-word copy (shipped) | +1 008 |

A code one byte different from the source is refused in 30 344 cycles (stage one) or 30 418
(stage two), tier 16. `tests/guard.rs` also runs 16 381- and 24 575-byte codes, which need a
second `POSEIDON2` call.

## Compatibility

**The guest is a single-contract EVM with a documented subset, not a mainnet-equivalent one, and
its gas is not mainnet gas.** [COMPATIBILITY.md](COMPATIBILITY.md) is the one place that says
what that means: every opcode byte and every precompile address, what the interpreter does with
it, what a translation does with it, and its gas against mainnet's; the fixed caps; and the open
question of calls between contracts. It is generated from `src/compat.rs`, and `tests/compat.rs`
runs all 256 opcode bytes through the interpreter and through a translation at both stages
against it, so the table cannot say something the code does not do.

[CYCLES.md](CYCLES.md) is the measured cost of each opcode in the interpreter image, in cycles
(`tests/cycles.rs` measures it on the committed `evm.bin`).

Before translating or proving a contract, scan it:

```console
$ evm2rv check ../guests-compiled/evm/contracts/erc20.runtime.hex
../guests-compiled/evm/contracts/erc20.runtime.hex: 1296 code bytes
0 always trap, 0 may trap, 0 trap in the interpreter only
10 SLOAD/SSTORE: their gas is not mainnet's (COMPATIBILITY.md)
3 undefined bytes (they halt on mainnet too; data embedded in the code reads as these)
clean: nothing in this code traps in a translation
```

`check` disassembles the code (push data skipped) and lists every opcode that always traps, traps
for some operands (a call runs only to a precompile; `RETURNDATACOPY`) or outside a version-2
context (the block-context opcodes, which run only in an `InvokeAt`), or runs in a translation but
not always in the interpreter (`CHAINID`, `ORIGIN`), with its offset and the table's reason. The
exit status is 0 when nothing traps, 1 when something always traps (or the code is over the size
cap), 2 when the only findings are conditional. `--interpreter` gives the verdict for the
interpreter instead of a translation. It is a scan, not a reachability analysis: an opcode in dead
code, or data that reads as one, is listed like any other.

## The observable contract

What must match the interpreter is the status, the eight output words and `gas_used`. The halt
kind is not public, and it may differ. Three divergences are accepted:

1. **The halt kind.** Static gas is charged once at the head of each block. A block that would
   halt midway for another reason can halt `OutOfGas` at its head instead, or the reverse. Both are
   status 2 with `gas_used` equal to the limit, so nothing observable changes.
2. **The translation runs a little more than the interpreter.** The interpreter traps on every
   call, and on `CHAINID` and `ORIGIN` outside a version-2 context. The translation implements
   `CHAINID` (the baked constant, under every context; the interpreter's, under version 2, is the
   context's chain id) and `ORIGIN`, and runs calls to the precompiles (addresses 1 to 9). A call
   on too shallow a stack underflows at the block head where the interpreter traps; both are
   status 2.
3. **The code guard.** Given code other than its source, the interpreter would run it. The
   translation refuses it (status 2, `gas_used` 0).

## Environment opcodes

| opcode | translation |
|---|---|
| `ADDRESS`, `CALLER`, `CALLVALUE`, `CALLDATASIZE`, `CODESIZE` | from the input vector, as the interpreter reads them |
| `CHAINID` | the `--chain-id` constant, baked in, so `hc` binds it, under every context; 2 gas |
| `ORIGIN` | `CALLER` (one call, no relayer); 2 gas |
| `NUMBER`, `TIMESTAMP`, `BASEFEE`, `GASPRICE`, `COINBASE`, `GASLIMIT`, `PREVRANDAO` | under a version-2 context (an `InvokeAt`), the word below, read from the public segment when the opcode runs; trap (status 2) under any other, as in the interpreter; 2 gas at the block head |
| `SELFBALANCE`, `BLOCKHASH`, `BLOBHASH`, `BLOBBASEFEE` | trap (status 2), as in the interpreter |

### Block context (constraint set 11, fullnode #168)

A private input word is bound only to the salted `H_IN`, which a verifier cannot open, so it
cannot carry a block fact: a prover could supply any. An `InvokeAt` transaction names a recent
closed block, the reference block, and the validator puts that block's facts into the public
segment the proof is checked against: context version 2 (fullnode `docs/evm-block-context.md`).
An EVM program is deployed with an empty public input, so the context starts at public word 8,
after the eight call-binding words. Both paths decode it with the same code, `evm_core::block`: the
interpreter directly, a translation through `evm-rt/evm_block.c` and `ffi::evm_block_context`,
linked only into a contract that uses one of these opcodes (one that uses none builds as before).

| opcode | under a version-2 context |
|---|---|
| `NUMBER` | the reference block's height (context words 11–12) |
| `TIMESTAMP` | its time in seconds (13–14): a recent block's, not "now", so a deadline check passes for up to the reference window after the deadline |
| `BASEFEE`, `GASPRICE` | the base units of RAND per unit of Rand gas the block's calls paid (17–18); no tips, so the two are equal |
| `PREVRANDAO` | the beacon value after the block's reveal, its 32 bytes (20–27) read big-endian; **traps** unless word 19 is 1 (a chain without the beacon). The prover chooses the reference block, so a contract must pin it (spec §11.4) |
| `COINBASE` | 0: fees go to the proposer's register entry, which has no EVM address |
| `GASLIMIT` | 2^32 − 1, the most gas one call's input can carry |
| `CHAINID` (interpreter only) | the genesis chain id (15–16) |
| `ORIGIN` (interpreter only) | `CALLER` |

Under any other context version — an ordinary `Invoke` is version 1 — each traps exactly as it did
before, with nothing charged first, so a version-1 run is unchanged to the halt kind. A `Call`'s
segment carries no context at all: the words are read only when one of these opcodes runs, so a
run that reaches none is unchanged, and a run that reaches one reads past the segment, which no
proof can show — there is no proof of it (it used to prove a status-2 halt).

## Calls and precompiles

- All four call opcodes (`CALL`, `CALLCODE`, `DELEGATECALL`, `STATICCALL`) run a precompile when
  the target is 1 to 9, with Shanghai gas. Any other target traps, as in the interpreter.
- A nonzero value on `CALL` or `CALLCODE` traps: there is no balance model.
- `modexp`'s base and modulus are capped at 1 024 bytes each. Past the cap, an affordable call
  halts `OutOfBounds`. The exponent is not capped.
- `RETURNDATASIZE` and `RETURNDATACOPY` read a real return-data buffer in a contract that calls.

All nine are software. Measured with one known answer each (`evm-rt/test/rv32-precompiles`):

| precompile | vector | cycles | fits 2^20? |
|---|---|---:|---|
| 1 ecrecover | ValidKey | 14 510 525 | no |
| 2 sha256 | "abc" / 200 bytes | 2 791 / 5 289 | yes |
| 3 ripemd160 | "abc" | 12 195 | yes |
| 4 identity | 100 bytes | 4 498 | yes |
| 5 modexp | nagydani-1-square / -1-pow0x10001 | 122 332 / 874 548 | yes |
| 5 modexp | eip_example1 (256-bit) | 11 170 118 | no |
| 5 modexp | nagydani-5-pow0x10001 (1 024-byte) | 55 076 950 | no |
| 6 bn256 add | chfast1 | 978 262 | yes, just |
| 7 bn256 mul | chfast1 | 3 060 669 | no |
| 8 bn256 pairing | 1 pair / 2 pairs | 1 853 656 212 / 2 019 000 469 | no |
| 9 blake2f | 12 rounds | 14 493 | yes (up to about 1 800 rounds) |

The ones over 2^20 cycles are correct but cannot be proven today. They are the coprocessor
backlog: ecrecover, bn256 mul, the pairing, modexp past small operands, and blake2f past about
1 800 rounds.

## Cycles against the interpreter

Measured with `rand-guest run` on the vectors `tests/parity.rs` and `tests/precompiles.rs` pin,
with the code guard in, under storage layout 2 (a depth-64 storage tree), with the call, the log
data and the gas in the public output, and after ZKVM-117's harness savings (below). Tier in
parentheses. The interpreter column is the block-context image (set 11, fullnode #168); the
translated columns are clang 23.1.1's from before it (on 23.1.4, the only clang the change was
measured with, each is 3 cycles more, which may be the compiler rather than the change).

| vector | interpreter | stage one | stage two |
|---|---:|---:|---:|
| `transfer(BOB, 250)` | 121 737 (18) | 73 922 (18) | **59 946** (16) |
| `approve(BOB, 5)` | 84 407 (18) | 46 766 (16) | **38 624** (16) |
| `transferFrom(ALICE, BOB, 100)` | 163 014 (18) | 106 241 (18) | **84 991** (18) |
| transfer of 5 000 of 1 000 (reverts) | 87 409 (18) | 45 331 (16) | 37 905 (16) |
| transfer, out of gas at 100 | 49 188 (16) | 23 073 (16) | 22 624 (16) |
| transfer, out of gas at 20 000 | 100 939 (18) | 58 633 (16) | 48 245 (16) |
| transfer, out of gas at 29 955 | 119 386 (18) | 71 307 (18) | 57 792 (16) |
| sha256 + ecrecover, underfunded | traps | 17 522 (16) | 16 458 (16) |

The transfer's row over time (interpreter / stage one / stage two):

| image | cycles | stage two's tier |
|---|---|---:|
| before ZKVM-103/104 | 124 606 / 80 211 / 66 235 | 18 |
| the call, log data and gas in the output (ZKVM-103/104) | 128 872 / 84 271 / 70 295 | 18 |
| the depth-64 storage tree (ZKVM-105) | 154 517 / 109 947 / 95 971 | 18 |
| the harness savings (ZKVM-117) | 120 892 / 73 922 / **59 946** | **16** |
| the block context (set 11, fullnode #168; interpreter only) | 121 737 / — / — | 16 |

> **Constraint set 11 moved every `hc`** (ZKVM-107: the Poseidon2 round constants are the Grain
> table, and `hc` is a Poseidon2 digest of the image's words), and set 11's block context
> (fullnode #168) moved them again. The interpreter rows and the ERC-20 rows below are the block
> context's (the ERC-20's from CI's clang-23.1.1 job, which is what `tests/parity.rs` pins; the
> text/data/prologue split in step 6 was not re-measured); the sha256 + ecrecover rows are still
> set 10's values — owed, with the pinned clang, before a chain admits that translation by `hc`.

| program | words | hc |
|---|---:|---|
| interpreter (`evm.bin`, layout 2) | 15 531 | `3869b014…8920720d` (set 11's block context; `7862f530…151eeb46`, 15 167 words, with set 11's Grain constants before it; `fae9d774…1f09bc0c` before set 11) |
| interpreter (`evm-v1.bin`, layout 1) | 15 474 | `040c01c8…f41193454` (the block context; `6215d629…3b64bfcdcf`, 15 111 words, before it; `7375f67a…794dca16` before set 11) |
| ERC-20, stage one | 12 083 | `043fb8c5922612352712b4e123c1a14f1def4c8c963a4b90bacc92bdd0c743e3` (`434cc062…65f6`, 12 076 words, before the block context) |
| ERC-20, stage two | 7 965 | `fd8dd3afdd785a991993c0f17dcca9a7c2c4b4c3bf213e2354d9aa1ffbf6e400` (`72ce1c04…77b8`, 7 958 words, before it) |
| sha256 + ecrecover contract, stage one | 12 451 | `d358b7db…d9676203` |
| sha256 + ecrecover contract, stage two | 12 223 | `666bfc23…a722154f` |

Before ZKVM-117: 16 588 (`cd700e35…993fa9e3`), 20 153 (`fb537193…a9d75818`), 12 783
(`c889e15c…cb9e`), 8 665 (`fa6d5ca0…493d`), 12 953 (`d8d21437…3c72e0d`) and 12 727
(`030fc0b0…18649ae`). Before ZKVM-105: 20 077 (`08740025…c4dc0f23`), 16 280 (`69052c42…e932`),
12 162 (`7c691208…8389`), 16 326 (`0b5bf110…4dc5a5`) and 16 100 (`9f9fe816…d12bc9`). Before
ZKVM-103/104: 19 637 (`31e682ec…6c289c3`), 15 804 (`9cdb79e7…48c8`), 11 686 (`a0feae92…8ff6`),
15 858 (`c8f00a09…f946fd`) and 15 630 (`011120cf…058c33`).

Stage two runs the three ERC-20 calls in 49.6% (`transfer`), 46.0% (`approve`) and 52.5%
(`transferFrom`) of the interpreter's cycles. Stage one takes 55.7 to 65.6%.

**The transfer is a tier-16 workload.** The tier bound is total rows (cycles plus digest rows: one per
four program words, one per four input words, and the public-input digest) at or under 65 535,
plus a separate Poseidon2 budget (`rand-guest/src/main.rs:244-248`). The transfer's 59 946 cycles
plus 2 353 digest rows are 62 299 total, 3 236 under tier 16's 65 535, and its 3 756 Poseidon2
permutations are well under tier 16's 8 192. `tests/parity.rs::the_stage_two_transfer_fits_tier_16`
pins that; `tests/proof.rs::the_stage_two_erc20_transfer_proves_at_tier_16` is the proof
(`#[ignore]`d: about 17 GB). `transferFrom` (three storage slots) is still tier 18.

### What the harness savings were (ZKVM-117)

None of them changes the machine: no syscall, no table, no constraint. Measured on the stage-two
transfer, 95 971 cycles before:

| change | where | what it removed |
|---|---|---|
| the Merkle walk hashes in place, in a window that slides down a buffer | `evm-core/src/storage.rs` (`walk`) | a fresh 17-word message per level and the digest copied out and back in: about 207 cycles a level became about 59, eight of them the syscall's own rows |
| a witness is read from the tape into its place | `storage.rs` (`read_witness`), `abi.rs` | a 2 KiB `Witness` built on the stack and moved: a `memcpy` call of 5 168 cycles per witness |
| the code's Keccak is baked into the image and published once the guard has passed | `evm2rv/src/guard.rs`, the shim, `abi.rs` (`run_call_with_executor_and_codehash`) | `keccak256` of the code in the guest: ten permutations and about 7 000 cycles for the ERC-20 |
| Keccak's last block is absorbed straight from the message | `evm-core/src/lib.rs` | a zeroed 136-byte staging buffer, a `memcpy` and 34 word loads on every call |
| the logs preimage buffer is not zeroed | `abi.rs` (`logs_hash`) | a 1 316-byte `memset` on every call |
| `U256` equality compares words | `evm-core/src/u256.rs` | a `memcmp` call of about 200 cycles per comparison (the interpreter's gain: 15 649 cycles of its transfer were that call) |
| the public-output preimage is built in the buffer it is hashed in | `abi.rs` | one copy of 83 words |

What is left of stage two's 59 946, from a pc histogram over the image's symbols (one-off, not
committed): the storage walks 15 459 (260 sponge calls at about 59), the remaining Keccak calls
about 4 000, decoding the input about 15 200 (1 441 words read one `READ_INPUT` at a time, the
1 296 bytes of code unpacked a byte at a time — the bulk-read syscall the issue names would take
most of that, and needs a constraint-set change), `memset`/`memcpy` calls about 9 600 (the
runtime's `u256` helpers, the zeroed `Outcome`, Keccak's zeroed state), and the contract and the
rest of the runtime the remainder.

### Where the cycles go

The harness is the code both images share: decoding the input, verifying each storage witness,
the ABI's own `keccak256` calls, and the output digest. The split below was measured on the
transfer with a one-off tool (Tasks 7 and 8; not committed): `llvm-nm` symbol ranges over each
cycle's pc, with each `memcpy`/`memset`/`memcmp` call charged to its caller. It was measured
before the code guard.

| transfer, before the guard | total | contract | harness | unclassified |
|---|---:|---:|---:|---:|
| stage one | 79 203 | 39 839 | 38 920 | 444 |
| stage two | 65 227 | 25 789 | 38 920 | 518 |

Stage two cuts the contract's own execution by 35%. The guard adds 1 008 cycles to the harness,
so the harness was about 39 928 of stage two's 66 235 cycles: about 60% of a transfer. ZKVM-103/104
added about 4 060 more to the harness (the calldata hash, the log-data hash, the longer digest) and
the depth-64 storage tree (ZKVM-105) about 25 700 more: about 69 700 of 95 971, 73%. ZKVM-117's
savings took about 36 000 back: about 34 000 of 59 946, **about 57% of a transfer**, taking the
contract's own 25 789 as unchanged (that split was not re-measured). No translation work can go below that floor.

## Proving

**Not yet proven on a 48 GB laptop.** A proof of the stage-one transfer (Task 7, before the
guard) was run once, with `Machine::new(FriProfile::Test).prove`, and the OS killed it:

| run | wall time | peak memory | result |
|---|---:|---:|---|
| stage-one transfer, tier 18 | 1 015.76 s | 24 706 367 488 bytes (24.7 GB) | SIGKILL, no proof |

No stage-two proof and no `evm.bin` proof were attempted. Both are deferred to a machine with at
least 64 GB. `tests/proof.rs` holds the two `#[ignore]`d tests, each with its exact command. The
prover is unchanged; this is a memory gap, not a correctness gap.

## Tests

`cargo +1.98.1 test --release` in this directory: 152 tests, plus the 2 ignored proofs.

| file | what |
|---|---|
| `blocks.rs` | the jumpdest rule, block splitting, static gas, stack bounds |
| `emit.rs` | the emitted C and the shim's files, for both stages |
| `lift.rs` | stage two's lifting: text tests, directed hazards, the 4 KiB frame cap |
| `opcodes.rs` | 34 directed opcode edges, both stages, and again under stage two with every push laundered |
| `parity.rs` | the ERC-20's 8 vectors, both stages, against `evm.bin` and the native interpreter; the hc pins; the guard refusing a one-byte change |
| `precompiles.rs` | a sha256 + ecrecover contract, end to end |
| `fuzz.rs` | 10 000 random programs, plain and opaque, both stages, against the interpreter; a sample through the real pipeline |
| `compat.rs` | the compatibility table against the code: all 256 opcode bytes on both paths and both stages, the conditional rows, every precompile address, the gas rules, the caps, `COMPATIBILITY.md`, and `evm2rv check` |
| `cycles.rs` | the cycles of each opcode in `evm.bin`, and `CYCLES.md` |
| `guard.rs` | the code guard's digest, and long codes through the real pipeline |
| `toolchain.rs` | the clang pin and its override |
