/* The block-context opcodes of a translated contract (fullnode #168): see `evm_rt.h`. The
 * decoding of the version-2 context is `evm-core/src/block.rs`, reached through `ffi.rs`, so a
 * translation pushes exactly what the interpreter pushes. */
#include "evm_rt.h"

void evm_block_ctx(uint32_t op, u256 *r) {
    if (evm_block_context(evm_host, op, r->l) != 0) {
        evm_halt(EVM_HALT_TRAP, op);
    }
}
