/* The block-context opcodes' runtime (`evm_block.c`, fullnode #168): `evm_block_ctx` writes the
 * word the Rust side's `evm_block_context` gives (`evm_core::block`, through `ffi.rs`), and halts
 * `Trap(op)` when it refuses — any context version but 2, PREVRANDAO without the beacon. The FFI
 * is not linked on the host: `evm_block_context` is the recording stub below, which answers for
 * TIMESTAMP and refuses everything else. Its gas (G_BASE) is the block head's, so none of it is
 * charged here; the opcode/value semantics are `research/tests/evm_interp.rs`'s and the
 * end-to-end runs `evm2rv/tests/compat.rs`'s and `parity_block.rs`'s. */
#include <string.h>
#include "test.h"

static void *seen_host;
static uint32_t seen_op, calls;

uint32_t evm_block_context(void *host, uint32_t op, uint32_t *out) {
    seen_host = host;
    seen_op = op;
    calls++;
    if (op != 0x42) return EVM_HALT_TRAP;
    for (int i = 0; i < 8; i++) out[i] = 0x1000u * (uint32_t)(i + 1) + op;
    return 0;
}

static const uint8_t CODE[1] = {0x00};
static uint32_t op;
static u256 dst;
static int host_token;

static void b_block(void) { evm_block_ctx(op, &dst); }

int block_tests(void) {
    /* The word comes back through the host pointer, limb by limb. */
    CHECK(evm_rt_init(CODE, 1, CODE, 0, 1000) == EVM_HALT_OK);
    evm_host = &host_token;
    memset(&dst, 0xee, sizeof dst);
    op = 0x42;
    calls = 0;
    CHECK(evm_rt_enter(b_block) == EVM_HALT_STOP);
    CHECK(calls == 1 && seen_op == 0x42 && seen_host == &host_token);
    for (int i = 0; i < 8; i++) CHECK(dst.l[i] == 0x1000u * (uint32_t)(i + 1) + 0x42);
    CHECK_EQ_U64(evm_gas_used(), 0);
    /* A refusal is Trap(op): the opcode as the halt's argument, the whole limit spent. */
    for (uint32_t o = 0x41; o <= 0x48; o++) {
        if (o == 0x42 || o == 0x46 || o == 0x47) continue;
        CHECK(evm_rt_init(CODE, 1, CODE, 0, 1000) == EVM_HALT_OK);
        op = o;
        CHECK(evm_rt_enter(b_block) == EVM_HALT_TRAP);
        CHECK(evm_halt_arg == o && seen_op == o);
        CHECK_EQ_U64(evm_gas_used(), 1000);
    }
    printf("block: done\n");
    return 0;
}
