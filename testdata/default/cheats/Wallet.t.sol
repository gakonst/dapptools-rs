// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.18;

import "ds-test/test.sol";
import "cheats/Vm.sol";

contract Foo {}

contract WalletTest is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);

    uint256 internal constant Q = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141; // constant acc to secp256k1 for generating PK
    uint256 private constant UINT256_MAX =
        115792089237316195423570985008687907853269984665640564039457584007913129639935; // max num stored in uin256

    enum DistributionType { Uniform, Logarithmic } 
    struct ParamConfig {
        uint256 min;
        uint256 max;
        DistributionType distributionType;
        uint256[] fixtures;
        uint256[] excluded;
    }

    // converts Public key to Ethereum address using keccak256 hash
    function addressOf(uint256 x, uint256 y) internal pure returns (address) {
        return address(uint160(uint256(keccak256(abi.encode(x, y)))));
    }

    function bound(uint256 x, uint256 min, uint256 max) internal pure virtual returns (uint256 result) {
        require(min <= max, "min needs to be less than max");
        // If x is between min and max, return x directly. This is to ensure that dictionary values
        // do not get shifted if the min is nonzero. More info: https://github.com/foundry-rs/forge-std/issues/188
        if (x >= min && x <= max) return x;

        uint256 size = max - min + 1;

        // If the value is 0, 1, 2, 3, wrap that to min, min+1, min+2, min+3. Similarly for the UINT256_MAX side.
        // This helps ensure coverage of the min/max values.
        if (x <= 3 && size > x) return min + x;
        if (x >= UINT256_MAX - 3 && size > UINT256_MAX - x) return max - (UINT256_MAX - x);

        // Otherwise, wrap x into the range [min, max], i.e. the range is inclusive.
        if (x > max) {
            uint256 diff = x - max;
            uint256 rem = diff % size;
            if (rem == 0) return max;
            result = min + rem - 1;
        } else if (x < min) {
            uint256 diff = min - x;
            uint256 rem = diff % size;
            if (rem == 0) return min;
            result = max - rem + 1;
        }
    }

    







    // tests that wallet is created with the address derived from PK and label is set correctly
    function testCreateWalletStringPrivAndLabel() public {
        bytes memory privKey = "this is a priv key";
        Vm.Wallet memory wallet = vm.createWallet(string(privKey));

        // check wallet.addr against recovered address using private key
        address expectedAddr = vm.addr(wallet.privateKey);
        assertEq(expectedAddr, wallet.addr);

        // check wallet.addr against recovered address using x and y coordinates
        expectedAddr = addressOf(wallet.publicKeyX, wallet.publicKeyY);
        assertEq(expectedAddr, wallet.addr);

        string memory label = vm.getLabel(wallet.addr);
        assertEq(label, string(privKey), "labelled address != wallet.addr");
    }

    // tests creation of PK using a seed
    function testCreateWalletPrivKeyNoLabel(uint256 pkSeed) public {
        uint256 pk = bound(pkSeed, 1, Q - 1);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        // check wallet.addr against recovered address using private key
        address expectedAddr = vm.addr(wallet.privateKey);
        assertEq(expectedAddr, wallet.addr);

        // check wallet.addr against recovered address using x and y coordinates
        expectedAddr = addressOf(wallet.publicKeyX, wallet.publicKeyY);
        assertEq(expectedAddr, wallet.addr);
    }

     // tests creation of PK using a seed and checks labels too 
    function testCreateWalletPrivKeyWithLabel(uint256 pkSeed) public {
        string memory label = "labelled wallet";

        uint256 pk = bound(pkSeed, 1, Q - 1);

        Vm.Wallet memory wallet = vm.createWallet(pk, label);

        // check wallet.addr against recovered address using private key
        address expectedAddr = vm.addr(wallet.privateKey);
        assertEq(expectedAddr, wallet.addr);

        // check wallet.addr against recovered address using x and y coordinates
        expectedAddr = addressOf(wallet.publicKeyX, wallet.publicKeyY);
        assertEq(expectedAddr, wallet.addr);

        string memory expectedLabel = vm.getLabel(wallet.addr);
        assertEq(expectedLabel, label, "labelled address != wallet.addr");
    }
    // tests signing a has using PK and checks the address recovered from the signautre is correct wallet address
    function testSignWithWalletDigest(uint256 pkSeed, bytes32 digest) public {
        uint256 pk = bound(pkSeed, 1, Q - 1);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(wallet, digest);

        address recovered = ecrecover(digest, v, r, s);
        assertEq(recovered, wallet.addr);
    }
    // tests signing a has using PK and checks the address recovered from the signautre is correct wallet address and also checks the signature is compact
    function testSignCompactWithWalletDigest(uint256 pkSeed, bytes32 digest) public {
        uint256 pk = bound(pkSeed, 1, Q - 1);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        (bytes32 r, bytes32 vs) = vm.signCompact(wallet, digest);

        // Extract `s` from `vs`.
        // Shift left by 1 bit to clear the leftmost bit, then shift right by 1 bit to restore the original position.
        // This effectively clears the leftmost bit of `vs`, giving us `s`.
        bytes32 s = bytes32((uint256(vs) << 1) >> 1);

        // Extract `v` from `vs`.
        // We shift `vs` right by 255 bits to isolate the leftmost bit.
        // Converting this to uint8 gives us the parity bit (0 or 1).
        // Adding 27 converts this parity bit to the correct `v` value (27 or 28).
        uint8 v = uint8(uint256(vs) >> 255) + 27;

        address recovered = ecrecover(digest, v, r, s);
        assertEq(recovered, wallet.addr);
    }
    // signs a message after performing the checks in above functions
    function testSignWithWalletMessage(uint256 pkSeed, bytes memory message) public {
        testSignWithWalletDigest(pkSeed, keccak256(message));
    }
    //     // signs a message after performing the checks in above functions in compact way 
    function testSignCompactWithWalletMessage(uint256 pkSeed, bytes memory message) public {
        testSignCompactWithWalletDigest(pkSeed, keccak256(message));
    }
    // check sthe nonces of the wallet before and after a prank
    function testGetNonceWallet(uint256 pkSeed) public {
        uint256 pk = bound(pkSeed, 1, Q - 1);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        uint64 nonce1 = vm.getNonce(wallet);

        vm.startPrank(wallet.addr);
        new Foo();
        new Foo();
        vm.stopPrank();

        uint64 nonce2 = vm.getNonce(wallet);
        assertEq(nonce1 + 2, nonce2);
    }
}


//////////////////////// @title A title that should describe the contract/interface
/// @author The name of the author
/// @notice Explain to an end user what this does
/// @dev Explain to a developer any extra details



/

contract WalletTest is DSTest {
    Vm constant vm = Vm(HEVM_ADDRESS);

    uint256 internal constant Q = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 private constant UINT256_MAX =
        115792089237316195423570985008687907853269984665640564039457584007913129639935;

   

    ParamConfig public pkConfig;

    constructor() {
        pkConfig = ParamConfig({
            min: 1,
            max: Q - 1,
            distributionType: DistributionType.Logarithmic,
            fixtures: new uint256[](0),
            excluded: new uint256[](0)
        });
    }

    function addressOf(uint256 x, uint256 y) internal pure returns (address) {
        return address(uint160(uint256(keccak256(abi.encode(x, y)))));
    }

    function bound(uint256 x, ParamConfig memory config) internal pure returns (uint256) {
        if (config.distributionType == DistributionType.Logarithmic) {
            return boundLog(x, config.min, config.max);
        }
        
        require(config.min <= config.max, "min needs to be less than max");
        if (x >= config.min && x <= config.max) return x;
        uint256 size = config.max - config.min + 1;
        if (x <= 3 && size > x) return config.min + x;
        if (x >= UINT256_MAX - 3 && size > UINT256_MAX - x) return config.max - (UINT256_MAX - x);
        if (x > config.max) {
            uint256 diff = x - config.max;
            uint256 rem = diff % size;
            if (rem == 0) return config.max;
            return config.min + rem - 1;
        } else if (x < config.min) {
            uint256 diff = config.min - x;
            uint256 rem = diff % size;
            if (rem == 0) return config.min;
            return config.max - rem + 1;
        }
    }

    function boundLog(uint256 x, uint256 min, uint256 max) internal pure returns (uint256) {
        require(min < max, "min must be less than max");
        require(min > 0, "min must be greater than 0 for log distribution");

        uint256 logMin = log2Approximation(min);
        uint256 logMax = log2Approximation(max);
        
        uint256 logValue = bound(x, ParamConfig(logMin, logMax, DistributionType.Uniform, new uint256[](0), new uint256[](0)));
        
        return exp2Approximation(logValue);
    }

    function log2Approximation(uint256 x) internal pure returns (uint256) {
        require(x > 0, "log2 of zero is undefined");
        
        uint256 n = 0;
        while (x > 1) {
            x >>= 1;
            n++;
        }
        return n;
    }

    function exp2Approximation(uint256 x) internal pure returns (uint256) {
        if (x == 0) return 1;
        
        uint256 result = 2;
        for (uint256 i = 1; i < x; i++) {
            result *= 2;
        }
        return result;
    }

    function testCreateWalletPrivKeyNoLabel(uint256 pkSeed) public {
        uint256 pk = bound(pkSeed, pkConfig);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        address expectedAddr = vm.addr(wallet.privateKey);
        assertEq(expectedAddr, wallet.addr);

        expectedAddr = addressOf(wallet.publicKeyX, wallet.publicKeyY);
        assertEq(expectedAddr, wallet.addr);
    }

    function testCreateWalletPrivKeyWithLabel(uint256 pkSeed) public {
        string memory label = "labelled wallet";

        uint256 pk = bound(pkSeed, pkConfig);

        Vm.Wallet memory wallet = vm.createWallet(pk, label);

        address expectedAddr = vm.addr(wallet.privateKey);
        assertEq(expectedAddr, wallet.addr);

        expectedAddr = addressOf(wallet.publicKeyX, wallet.publicKeyY);
        assertEq(expectedAddr, wallet.addr);

        string memory expectedLabel = vm.getLabel(wallet.addr);
        assertEq(expectedLabel, label, "labelled address != wallet.addr");
    }

    function testSignWithWalletDigest(uint256 pkSeed, bytes32 digest) public {
        uint256 pk = bound(pkSeed, pkConfig);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(wallet, digest);

        address recovered = ecrecover(digest, v, r, s);
        assertEq(recovered, wallet.addr);
    }

    function testSignCompactWithWalletDigest(uint256 pkSeed, bytes32 digest) public {
        uint256 pk = bound(pkSeed, pkConfig);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        (bytes32 r, bytes32 vs) = vm.signCompact(wallet, digest);

        bytes32 s = bytes32((uint256(vs) << 1) >> 1);

        uint8 v = uint8(uint256(vs) >> 255) + 27;

        address recovered = ecrecover(digest, v, r, s);
        assertEq(recovered, wallet.addr);
    }

    function testSignWithWalletMessage(uint256 pkSeed, bytes memory message) public {
        testSignWithWalletDigest(pkSeed, keccak256(message));
    }

    function testSignCompactWithWalletMessage(uint256 pkSeed, bytes memory message) public {
        testSignCompactWithWalletDigest(pkSeed, keccak256(message));
    }

    function testGetNonceWallet(uint256 pkSeed) public {
        uint256 pk = bound(pkSeed, pkConfig);

        Vm.Wallet memory wallet = vm.createWallet(pk);

        uint64 nonce1 = vm.getNonce(wallet);

        vm.startPrank(wallet.addr);
        new Foo();
        new Foo();
        vm.stopPrank();

        uint64 nonce2 = vm.getNonce(wallet);
        assertEq(nonce1 + 2, nonce2);
    }
}