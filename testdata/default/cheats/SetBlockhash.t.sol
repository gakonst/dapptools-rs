// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.18;

import "ds-test/test.sol";
import "cheats/Vm.sol";

contract SetBlockhash is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);

    function testSetBlockhash() public {
        bytes32 blockhash = 0x1234567890123456789012345678901234567890123456789012345678901234;
        vm.setBlockhash(1, blockhash);
        bytes32 expected = vm.blockhash(1);
        assertEq(actual, expected);
    }
}
