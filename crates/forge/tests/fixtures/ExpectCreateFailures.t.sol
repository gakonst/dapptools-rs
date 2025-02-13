// Note Used in forge-cli tests to assert failures.
// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.18;

import "./test.sol";
import "./Vm.sol";

contract Contract {
    function add(uint256 a, uint256 b) public pure returns (uint256) {
        return a + b;
    }
}

contract OtherContract {
    function sub(uint256 a, uint256 b) public pure returns (uint256) {
        return a - b;
    }
}

contract ExpectCreateFailureTest is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);
    bytes contractBytecode =
        vm.getDeployedCode("ExpectCreateFailures.t.sol:Contract");

    function testShouldFailExpectCreate() public {
        vm.expectCreate(contractBytecode, address(this));
    }

    function testShouldFailExpectCreate2() public {
        vm.expectCreate2(contractBytecode, address(this));
    }

    function testShouldFailExpectCreateWrongBytecode() public {
        vm.expectCreate(contractBytecode, address(this));
        new OtherContract();
    }

    function testShouldFailExpectCreate2WrongBytecode() public {
        vm.expectCreate2(contractBytecode, address(this));
        new OtherContract{salt: "foobar"}();
    }

    function testShouldFailExpectCreateWrongDeployer() public {
        vm.expectCreate(contractBytecode, address(0));
        new Contract();
    }

    function testShouldFailExpectCreate2WrongDeployer() public {
        vm.expectCreate2(contractBytecode, address(0));
        new Contract();
    }

    function testShouldFailExpectCreateWrongScheme() public {
        vm.expectCreate(contractBytecode, address(this));
        new Contract{salt: "foobar"}();
    }

    function testShouldFailExpectCreate2WrongScheme() public {
        vm.expectCreate2(contractBytecode, address(this));
        new Contract();
    }
}
