// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.18;

import "ds-test/test.sol";
import "cheats/Vm.sol";

contract Contract {
    function add(uint256 a, uint256 b) public pure returns (uint256) {
        return a + b;
    }
}

contract ExpectCreateTest is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);
    bytes bytecode = vm.getDeployedCode("cheats/ExpectCreate.t.sol:Contract");

    function testExpectCreate() public {
        vm.expectCreate(bytecode, address(this));
        new Contract();
    }

    function testExpectCreate2() public {
        vm.expectCreate2(bytecode, address(this));
        new Contract{salt: "foo"}();
    }
}
