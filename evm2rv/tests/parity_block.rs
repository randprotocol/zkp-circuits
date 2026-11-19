//! The block-context opcodes end to end (fullnode #168, constraint set 11): a hand-written
//! contract that returns every block word a version-2 context defines, translated by `evm2rv` at
//! both stages, built into images (`evm_block.c` linked, the shim's `READ_PUBLIC` behind the host)
//! and run on the emulator over an invoke's **public segment**, against the interpreter natively
//! and as `evm.bin`.
//!
//! * Under a version-2 context with the beacon every opcode runs: the eight public words agree,
//!   the return-data hash over the nine words among them.
//! * Without the beacon `PREVRANDAO` traps; under a version-1 context the first block opcode
//!   traps; both status 2, the same words on every path.
//! * With empty calldata the contract returns 42 and reaches none of them: the same words over
//!   every segment, an empty one (a `Call`'s) included. With calldata and an empty segment the run
//!   reads past it, which the machine refuses: there is no proof of it.
//!
//! `CHAINID` is not in the contract: a translation's is the `--chain-id` constant, and these
//! builds give none (`compat.rs` holds the interpreter's against the context word).

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{build_shim, root, scrubbed, rand_guest, STAGES};
use evm2rv::emit::Stage;
use evm_core::u256::U256;
use rand_zkvm::evm::{v1_segment, BlockFacts, EvmCall, SparseTree, ALICE, TOKEN};

/// GASPRICE first (ORIGIN runs in a translation under any context, so it must not be the first
/// to trap), then the rest, each word stored at 32·k; the eight words returned. Empty calldata returns
/// 42 instead, before any of them.
const OPS: [u8; 8] = [0x3a, 0x41, 0x42, 0x43, 0x44, 0x45, 0x48, 0x32];

fn contract() -> Vec<u8> {
    // 0: CALLDATASIZE PUSH1 12 JUMPI; 4: PUSH1 42 PUSH0 MSTORE PUSH1 32 PUSH0 RETURN; 12: JUMPDEST
    let mut c = vec![0x36, 0x60, 0x0c, 0x57, 0x60, 0x2a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3, 0x5b];
    for (k, op) in OPS.into_iter().enumerate() {
        c.extend_from_slice(&[op, 0x60, 32 * k as u8, 0x52]);
    }
    let len = (32 * OPS.len() as u16).to_be_bytes();
    c.extend_from_slice(&[0x61, len[0], len[1], 0x5f, 0xf3]);
    c
}

fn images(stage: Stage) -> &'static PathBuf {
    static CELLS: [OnceLock<PathBuf>; 2] = [const { OnceLock::new() }; 2];
    CELLS[matches!(stage, Stage::Two) as usize].get_or_init(|| {
        let base = root().join("evm2rv/target/parity");
        std::fs::create_dir_all(&base).unwrap();
        let hex = base.join("block.runtime.hex");
        std::fs::write(&hex, hex::encode(contract())).unwrap();
        let dir = base.join(format!("block-stage{}", matches!(stage, Stage::Two) as u8 + 1));
        build_shim(&hex, &dir, "block-evm2rv", false, stage).0
    })
}

fn call(calldata: Vec<u8>) -> EvmCall {
    EvmCall {
        code: contract(),
        calldata,
        address: TOKEN,
        caller: ALICE,
        callvalue: U256::ZERO,
        gas_limit: 100_000,
        context: [9; 8],
        tree: SparseTree::new(),
        touched: vec![],
    }
}

/// `rand-guest run image --input … --public …`: the eight output words, or `Err(stdout+stderr)`
/// when the machine refused the run.
fn run_on(image: &Path, inputs: &[u32], public: &[u32]) -> Result<[u32; 8], String> {
    let mut c = scrubbed(rand_guest());
    c.arg("run").arg(image).arg("--input").args(inputs.iter().map(u32::to_string));
    if !public.is_empty() {
        c.arg("--public").args(public.iter().map(u32::to_string));
    }
    let o = c.output().unwrap();
    let s = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let mut out = [0u32; 8];
    for (i, w) in out.iter_mut().enumerate() {
        let prefix = format!("out[{i}] = ");
        match s.lines().find_map(|l| l.strip_prefix(&prefix)) {
            Some(v) if o.status.success() => *w = v.parse().unwrap(),
            _ => return Err(s),
        }
    }
    Ok(out)
}

fn facts(randomness: Option<[u8; 32]>) -> BlockFacts {
    BlockFacts {
        height: 0x2_0000_0abc,
        timestamp_ms: 1_795_123_456_789,
        chain_id: 22,
        basefee: 0x1_0000_0003,
        randomness,
    }
}

#[test]
fn the_block_context_opcodes_match_the_interpreter_over_every_segment() {
    let binding = [0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8];
    let mut beacon = [0u8; 32];
    for (i, b) in beacon.iter_mut().enumerate() {
        *b = 0x5a ^ (i as u8).wrapping_mul(13);
    }
    let with = facts(Some(beacon)).segment(&binding);
    let without = facts(None).segment(&binding);
    let v1 = v1_segment(&binding);
    let evm = root().join("guests-compiled/bin/evm.bin");

    let block = call(vec![1]);
    let plain = call(vec![]);
    let cases: [(&str, &EvmCall, &[u32], u32); 7] = [
        ("version 2", &block, &with, 1),
        ("version 2, no beacon", &block, &without, 2),
        ("version 1", &block, &v1, 2),
        ("plain, version 2", &plain, &with, 1),
        ("plain, version 1", &plain, &v1, 1),
        ("plain, no context (a Call)", &plain, &[], 1),
        ("plain, the binding alone", &plain, &binding, 1),
    ];
    for (name, c, seg, status) in cases {
        let words = c.input_words();
        let (want, o, _) = c.expected_on(seg);
        assert_eq!(want[0], status, "{name}: the oracle");
        assert_eq!(run_on(&evm, &words, seg).unwrap(), want, "{name}: evm.bin");
        for stage in STAGES {
            assert_eq!(run_on(images(stage), &words, seg).unwrap(), want, "{name} ({stage:?})");
        }
        if name == "version 2" {
            // The nine words themselves, as the interpreter returned them.
            let w = |k: usize| U256::from_be_slice(&o.ret[32 * k..32 * k + 32]);
            assert_eq!(o.ret_len, 32 * OPS.len());
            assert_eq!(w(0), U256::from_u64(0x1_0000_0003), "GASPRICE = BASEFEE");
            assert_eq!(w(1), U256::ZERO, "COINBASE");
            assert_eq!(w(2), U256::from_u64(1_795_123_456), "TIMESTAMP in seconds");
            assert_eq!(w(3), U256::from_u64(0x2_0000_0abc), "NUMBER = ref_height");
            assert_eq!(w(4), U256::from_be_bytes(&beacon), "PREVRANDAO");
            assert_eq!(w(5), U256::from_u64(u32::MAX as u64), "GASLIMIT");
            assert_eq!(w(6), U256::from_u64(0x1_0000_0003), "BASEFEE");
            assert_eq!(w(7), ALICE, "ORIGIN = CALLER");
        }
        eprintln!("{name}: status {} ({:?}, gas {})", want[0], o.halt, o.gas_used);
    }

    // A `Call`'s segment has no context: reaching a block opcode reads past it, and the machine
    // refuses the run on every path — there is no proof of it.
    let words = block.input_words();
    for image in [&evm, images(Stage::One), images(Stage::Two)] {
        let e = run_on(image, &words, &[]).unwrap_err();
        assert!(e.contains("PublicIndex(8)"), "{}: {e}", image.display());
    }
}
