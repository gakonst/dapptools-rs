// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.18;

import "ds-test/test.sol";
import "cheats/Vm.sol";

contract RecordGasTest is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);

    function testRecordGasA() public {
        _burn(1);

        emit log_named_uint("gas A", vm.lastGasUsed());

        _burn(100);
        emit log_named_uint("gas B", vm.lastGasUsed());

        _burn(10000);
        emit log_named_uint("gas C", vm.lastGasUsed());
    }

    function _burn(uint256 x) internal pure {
        // Source: https://github.com/vectorized/solady/blob/main/src/utils/GasBurnerLib.sol
        /// @solidity memory-safe-assembly
        assembly {
            mstore(0x10, or(1, x))
            let n := mul(gt(x, 120), div(x, 91))
            // We use keccak256 instead of blake2f precompile for better widespread compatibility.
            for {
                let i := 0
            } iszero(eq(i, n)) {
                i := add(i, 1)
            } {
                mstore(0x10, keccak256(0x10, 0x10)) // Yes.
            }
            if iszero(mload(0x10)) {
                invalid()
            }
        }
    }
}
