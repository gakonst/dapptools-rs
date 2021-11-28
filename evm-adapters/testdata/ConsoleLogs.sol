pragma solidity ^0.8.0;

import "./console.sol";

contract ConsoleLogs {
    function test_log() public {
		console.log(0x1111111111111111111111111111111111111111);
		console.log("Hi");
		console.log("Hi", "Hi");
    }
	function test_log_elsewhere() public {
		OtherContract otherContract = new OtherContract();
		otherContract.test_log();
    }
}

contract OtherContract {
    function test_log() public {
		console.log(0x1111111111111111111111111111111111111111);
		console.log("Hi");
		console.log("Hi", "Hi");
    }
}