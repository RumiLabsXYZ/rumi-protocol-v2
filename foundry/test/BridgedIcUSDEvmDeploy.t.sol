// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {DeployBridgedIcUSDEvm} from "../script/DeployBridgedIcUSDEvm.s.sol";
import {DeployBridgedIcUSD} from "../script/DeployBridgedIcUSD.s.sol";
import {BridgedIcUSD} from "../src/BridgedIcUSD.sol";

/// @dev Exposes the sibling script's internal `deploy` and replaces its environment access
/// with an in-memory table, so these tests never touch the process-wide environment that
/// other test contracts also use. Everything runs against the in-memory Foundry EVM only:
/// no RPC URL, no private key, nothing is ever sent to a network (`vm.startBroadcast` in a
/// test just records transactions).
contract EvmDeployHarness is DeployBridgedIcUSDEvm {
    mapping(bytes32 => string) private values;

    function setValue(string memory name, string memory value) external {
        values[keccak256(bytes(name))] = value;
    }

    function clearValue(string memory name) external {
        delete values[keccak256(bytes(name))];
    }

    function deployPublic(Config memory cfg) external returns (BridgedIcUSD) {
        return deploy(cfg);
    }

    function _raw(string memory name) private view returns (string memory v, bool present) {
        v = values[keccak256(bytes(name))];
        present = bytes(v).length != 0;
    }

    function _envUint(string memory name) internal view override returns (uint256) {
        (string memory v, bool present) = _raw(name);
        require(present, string.concat("env var not set: ", name));
        return vm.parseUint(v);
    }

    function _envAddress(string memory name) internal view override returns (address) {
        (string memory v, bool present) = _raw(name);
        require(present, string.concat("env var not set: ", name));
        return vm.parseAddress(v);
    }

    function _envAddressOr(string memory name, address fallbackValue) internal view override returns (address) {
        (string memory v, bool present) = _raw(name);
        return present ? vm.parseAddress(v) : fallbackValue;
    }

    function _envBoolOr(string memory name, bool fallbackValue) internal view override returns (bool) {
        (string memory v, bool present) = _raw(name);
        return present ? vm.parseBool(v) : fallbackValue;
    }
}

/// @dev The Base script, to compare what it deploys with what the sibling deploys.
contract BaseDeployHarness is DeployBridgedIcUSD {
    function deployPublic(Config memory cfg) external returns (BridgedIcUSD) {
        return deploy(cfg);
    }
}

contract BridgedIcUSDEvmDeployTest is Test {
    EvmDeployHarness script;

    address admin = address(0xA11CE);
    address minter = address(0xB0B);
    address guardian = address(0x6A2D);

    uint256 constant ETHEREUM = 1;
    uint256 constant ETHEREUM_SEPOLIA = 11155111;
    uint256 constant ARBITRUM_ONE = 42161;
    uint256 constant ARBITRUM_SEPOLIA = 421614;
    uint256 constant MONAD = 143;
    uint256 constant MONAD_TESTNET = 10143;
    uint256 constant LOCAL = 31337;

    string constant UNSUPPORTED =
        "unsupported chain id (allowed: 11155111, 421614, 10143, 31337, and 1, 42161, 143 with their ALLOW_<CHAIN>_MAINNET flag)";

    function setUp() public {
        script = new EvmDeployHarness();
        vm.chainId(LOCAL);
    }

    function _cfg() internal view returns (DeployBridgedIcUSDEvm.Config memory cfg) {
        cfg = DeployBridgedIcUSDEvm.Config({
            expectedChainId: block.chainid,
            allowEthereumMainnet: false,
            allowArbitrumMainnet: false,
            allowMonadMainnet: false,
            admin: admin,
            minter: minter,
            guardian: guardian,
            adminDelaySeconds: 2 days,
            supplyCapE8s: 1_000_000e8,
            mintWindowLimitE8s: 100_000e8,
            mintWindowSeconds: 1 days,
            minBridgeOutE8s: 1e6
        });
    }

    function _testnets() internal pure returns (uint256[4] memory) {
        return [ETHEREUM_SEPOLIA, ARBITRUM_SEPOLIA, MONAD_TESTNET, LOCAL];
    }

    // ------------------------------------------------------------------
    // Happy paths
    // ------------------------------------------------------------------

    function test_deploys_with_intended_roles_on_every_allowed_test_chain() public {
        uint256[4] memory chains = _testnets();
        for (uint256 i = 0; i < chains.length; i++) {
            vm.chainId(chains[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            BridgedIcUSD t = script.deployPublic(cfg);

            assertEq(t.decimals(), 8);
            assertEq(t.VERSION(), "rumi-base-icusd-bridge-v1");
            assertEq(t.defaultAdmin(), admin);
            assertEq(t.defaultAdminDelay(), 2 days);
            assertEq(t.supplyCap(), 1_000_000e8);
            assertEq(t.mintWindowLimit(), 100_000e8);
            assertEq(t.mintWindowSeconds(), 1 days);
            assertEq(t.minBridgeOut(), 1e6);

            bytes32 adminRole = t.DEFAULT_ADMIN_ROLE();
            bytes32 minterRole = t.MINTER_ROLE();
            bytes32 pauserRole = t.PAUSER_ROLE();
            assertTrue(t.hasRole(adminRole, admin));
            assertTrue(t.hasRole(pauserRole, admin));
            assertFalse(t.hasRole(minterRole, admin));
            assertTrue(t.hasRole(minterRole, minter));
            assertTrue(t.hasRole(pauserRole, minter));
            assertFalse(t.hasRole(adminRole, minter));
            assertTrue(t.hasRole(pauserRole, guardian));
            assertFalse(t.hasRole(adminRole, guardian));
            assertFalse(t.hasRole(minterRole, guardian));

            // Neither the script, its caller nor the broadcaster holds anything.
            vm.startBroadcast();
            (, address broadcaster,) = vm.readCallers();
            vm.stopBroadcast();
            address[3] memory deployers = [address(script), address(this), broadcaster];
            for (uint256 j = 0; j < deployers.length; j++) {
                assertFalse(t.hasRole(adminRole, deployers[j]));
                assertFalse(t.hasRole(minterRole, deployers[j]));
                assertFalse(t.hasRole(pauserRole, deployers[j]));
            }
        }
    }

    function test_deploys_without_guardian() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.guardian = address(0);
        BridgedIcUSD t = script.deployPublic(cfg);
        assertFalse(t.hasRole(t.PAUSER_ROLE(), address(0)));
        assertTrue(t.hasRole(t.PAUSER_ROLE(), admin));
        assertTrue(t.hasRole(t.PAUSER_ROLE(), minter));
    }

    function test_each_mainnet_deploys_with_its_own_flag() public {
        // (chain id, which flag is raised)
        uint256[3] memory chains = [ETHEREUM, ARBITRUM_ONE, MONAD];
        for (uint256 i = 0; i < chains.length; i++) {
            vm.chainId(chains[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            cfg.allowEthereumMainnet = i == 0;
            cfg.allowArbitrumMainnet = i == 1;
            cfg.allowMonadMainnet = i == 2;
            // Validation passes; the deploy itself runs on the local in-memory EVM only.
            script.validateConfig(cfg);
            BridgedIcUSD t = script.deployPublic(cfg);
            assertEq(t.decimals(), 8);
            assertEq(t.VERSION(), "rumi-base-icusd-bridge-v1");
        }
    }

    // ------------------------------------------------------------------
    // Identical bytecode (D5)
    // ------------------------------------------------------------------

    function test_bytecode_is_identical_on_every_chain_and_to_the_base_script() public {
        // What the Base script deploys on Base Sepolia is the reference.
        BaseDeployHarness baseScript = new BaseDeployHarness();
        vm.chainId(84532);
        DeployBridgedIcUSD.Config memory baseCfg = DeployBridgedIcUSD.Config({
            expectedChainId: 84532,
            allowBaseMainnet: false,
            admin: admin,
            minter: minter,
            guardian: guardian,
            adminDelaySeconds: 2 days,
            supplyCapE8s: 1_000_000e8,
            mintWindowLimitE8s: 100_000e8,
            mintWindowSeconds: 1 days,
            minBridgeOutE8s: 1e6
        });
        BridgedIcUSD baseToken = baseScript.deployPublic(baseCfg);
        bytes32 baseHash = address(baseToken).codehash;
        assertEq(baseHash, keccak256(type(BridgedIcUSD).runtimeCode));

        uint256[7] memory chains =
            [ETHEREUM_SEPOLIA, ARBITRUM_SEPOLIA, MONAD_TESTNET, LOCAL, ETHEREUM, ARBITRUM_ONE, MONAD];
        for (uint256 i = 0; i < chains.length; i++) {
            vm.chainId(chains[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            cfg.allowEthereumMainnet = true;
            cfg.allowArbitrumMainnet = true;
            cfg.allowMonadMainnet = true;
            BridgedIcUSD t = script.deployPublic(cfg);
            assertEq(address(t).codehash, baseHash, "runtime code differs from the Base deploy");
            assertEq(t.BRIDGE_OUT_DOMAIN(), baseToken.BRIDGE_OUT_DOMAIN());
            assertEq(t.VERSION(), baseToken.VERSION());
            assertEq(t.name(), baseToken.name());
            assertEq(t.symbol(), baseToken.symbol());
        }
    }

    function test_verify_deployment_rejects_different_runtime_code() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        BridgedIcUSD t = script.deployPublic(cfg);
        script.verifyDeployment(t, cfg, address(this));

        // Replace the code at the token address with a different (but non-empty) program.
        vm.etch(address(t), hex"6001600055");
        vm.expectRevert("runtime bytecode differs from BridgedIcUSD");
        script.verifyDeployment(t, cfg, address(this));
    }

    // ------------------------------------------------------------------
    // Chain guard
    // ------------------------------------------------------------------

    function test_rejects_expected_chain_id_mismatch() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.expectedChainId = ETHEREUM_SEPOLIA; // running on 31337
        vm.expectRevert("EXPECTED_CHAIN_ID does not match block.chainid");
        script.deployPublic(cfg);

        // The mismatch is reported before the mainnet flag: a wrong --rpc-url never gets as far
        // as asking about the flag.
        vm.chainId(ETHEREUM);
        cfg = _cfg();
        cfg.expectedChainId = ARBITRUM_ONE;
        cfg.allowEthereumMainnet = true;
        vm.expectRevert("EXPECTED_CHAIN_ID does not match block.chainid");
        script.deployPublic(cfg);
    }

    function test_rejects_each_mainnet_without_its_flag() public {
        vm.chainId(ETHEREUM);
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        vm.expectRevert("Ethereum mainnet (1) requires ALLOW_ETHEREUM_MAINNET=true");
        script.deployPublic(cfg);

        vm.chainId(ARBITRUM_ONE);
        cfg = _cfg();
        vm.expectRevert("Arbitrum One (42161) requires ALLOW_ARBITRUM_MAINNET=true");
        script.deployPublic(cfg);

        vm.chainId(MONAD);
        cfg = _cfg();
        vm.expectRevert("Monad mainnet (143) requires ALLOW_MONAD_MAINNET=true");
        script.deployPublic(cfg);
    }

    function test_a_mainnet_flag_unlocks_only_its_own_chain() public {
        // The other two flags raised, the one that matters left down.
        vm.chainId(ETHEREUM);
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.allowArbitrumMainnet = true;
        cfg.allowMonadMainnet = true;
        vm.expectRevert("Ethereum mainnet (1) requires ALLOW_ETHEREUM_MAINNET=true");
        script.deployPublic(cfg);

        vm.chainId(ARBITRUM_ONE);
        cfg = _cfg();
        cfg.allowEthereumMainnet = true;
        cfg.allowMonadMainnet = true;
        vm.expectRevert("Arbitrum One (42161) requires ALLOW_ARBITRUM_MAINNET=true");
        script.deployPublic(cfg);

        vm.chainId(MONAD);
        cfg = _cfg();
        cfg.allowEthereumMainnet = true;
        cfg.allowArbitrumMainnet = true;
        vm.expectRevert("Monad mainnet (143) requires ALLOW_MONAD_MAINNET=true");
        script.deployPublic(cfg);
    }

    function test_flags_do_not_matter_on_test_chains_and_unlock_nothing_else() public {
        // Raised flags change nothing on a test chain.
        uint256[4] memory chains = _testnets();
        for (uint256 i = 0; i < chains.length; i++) {
            vm.chainId(chains[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            cfg.allowEthereumMainnet = true;
            cfg.allowArbitrumMainnet = true;
            cfg.allowMonadMainnet = true;
            script.validateConfig(cfg);
        }
        // And no flag unlocks a chain outside the list.
        uint256[11] memory rejected = [uint256(10), 56, 137, 8454, 84531, 4663, 46630, 999, 998, 1030, 560048];
        for (uint256 i = 0; i < rejected.length; i++) {
            vm.chainId(rejected[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            cfg.allowEthereumMainnet = true;
            cfg.allowArbitrumMainnet = true;
            cfg.allowMonadMainnet = true;
            vm.expectRevert(bytes(UNSUPPORTED));
            script.deployPublic(cfg);
        }
    }

    function test_base_chains_are_not_deployed_by_this_script() public {
        uint256[2] memory baseChains = [uint256(8453), 84532];
        for (uint256 i = 0; i < baseChains.length; i++) {
            vm.chainId(baseChains[i]);
            DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
            cfg.allowEthereumMainnet = true;
            cfg.allowArbitrumMainnet = true;
            cfg.allowMonadMainnet = true;
            vm.expectRevert("Base chains deploy with DeployBridgedIcUSD, not this script");
            script.deployPublic(cfg);
        }
    }

    // ------------------------------------------------------------------
    // Role and parameter validation
    // ------------------------------------------------------------------

    function test_rejects_zero_or_equal_roles() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.admin = address(0);
        vm.expectRevert("admin is the zero address");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.minter = address(0);
        vm.expectRevert("minter is the zero address");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.minter = cfg.admin;
        vm.expectRevert("admin and minter must be different addresses");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.guardian = cfg.admin;
        vm.expectRevert("guardian must be distinct from admin and minter");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.guardian = cfg.minter;
        vm.expectRevert("guardian must be distinct from admin and minter");
        script.deployPublic(cfg);
    }

    function test_rejects_zero_economic_parameters() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.mintWindowSeconds = 0;
        vm.expectRevert("mint window seconds is zero");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.supplyCapE8s = 0;
        vm.expectRevert("supply cap is zero");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.mintWindowLimitE8s = 0;
        vm.expectRevert("mint window limit is zero");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.minBridgeOutE8s = 0;
        vm.expectRevert("min bridge-out is zero");
        script.deployPublic(cfg);
    }

    function test_rejects_a_deployer_that_is_a_role_holder() public {
        // msg.sender of the script call is this test contract.
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        cfg.admin = address(this);
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.minter = address(this);
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.guardian = address(this);
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);

        // The address that actually signs (`--account` / `--private-key` at run time).
        vm.startBroadcast();
        (, address broadcaster,) = vm.readCallers();
        vm.stopBroadcast();
        cfg = _cfg();
        cfg.admin = broadcaster;
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);
    }

    // ------------------------------------------------------------------
    // Post-deploy verification catches a wrongly configured token
    // ------------------------------------------------------------------

    function test_verify_deployment_detects_mismatches() public {
        DeployBridgedIcUSDEvm.Config memory cfg = _cfg();
        BridgedIcUSD t = new BridgedIcUSD(
            admin,
            cfg.adminDelaySeconds,
            minter,
            guardian,
            cfg.supplyCapE8s,
            cfg.mintWindowLimitE8s,
            cfg.mintWindowSeconds,
            cfg.minBridgeOutE8s
        );
        script.verifyDeployment(t, cfg, address(this));

        DeployBridgedIcUSDEvm.Config memory bad = _cfg();
        bad.supplyCapE8s = cfg.supplyCapE8s + 1;
        vm.expectRevert("supplyCap mismatch");
        script.verifyDeployment(t, bad, address(this));

        bad = _cfg();
        bad.minter = address(0xBAD);
        vm.expectRevert("minter lacks MINTER_ROLE");
        script.verifyDeployment(t, bad, address(this));

        vm.expectRevert("deployer holds DEFAULT_ADMIN_ROLE");
        script.verifyDeployment(t, cfg, admin);
        vm.expectRevert("deployer holds MINTER_ROLE");
        script.verifyDeployment(t, cfg, minter);

        // An extra role is noticed: the guardian given MINTER_ROLE.
        bytes32 minterRole = t.MINTER_ROLE();
        vm.prank(admin);
        t.grantRole(minterRole, guardian);
        vm.expectRevert("guardian holds MINTER_ROLE");
        script.verifyDeployment(t, cfg, address(this));
    }

    // ------------------------------------------------------------------
    // Environment loading, through the in-memory table (no process-wide env)
    // ------------------------------------------------------------------

    function _fill(string memory chainId, string memory delay, string memory windowSeconds) internal {
        script.setValue("EXPECTED_CHAIN_ID", chainId);
        script.setValue("BRIDGE_ADMIN_ADDR", vm.toString(admin));
        script.setValue("BRIDGE_MINTER_ADDR", vm.toString(minter));
        script.setValue("BRIDGE_ADMIN_DELAY_SECONDS", delay);
        script.setValue("BRIDGE_SUPPLY_CAP_E8S", "1000000000000");
        script.setValue("BRIDGE_MINT_WINDOW_LIMIT_E8S", "100000000000");
        script.setValue("BRIDGE_MINT_WINDOW_SECONDS", windowSeconds);
        script.setValue("BRIDGE_MIN_BRIDGE_OUT_E8S", "1000000");
    }

    function test_env_loading_reads_each_mainnet_flag_separately() public {
        // Required variables missing -> loadConfig reverts.
        vm.expectRevert("env var not set: EXPECTED_CHAIN_ID");
        script.loadConfig();

        // Flags and the guardian default to off / none.
        _fill("31337", "172800", "86400");
        DeployBridgedIcUSDEvm.Config memory cfg = script.loadConfig();
        assertEq(cfg.expectedChainId, 31337);
        assertFalse(cfg.allowEthereumMainnet);
        assertFalse(cfg.allowArbitrumMainnet);
        assertFalse(cfg.allowMonadMainnet);
        assertEq(cfg.admin, admin);
        assertEq(cfg.minter, minter);
        assertEq(cfg.guardian, address(0));
        assertEq(cfg.adminDelaySeconds, 172800);
        assertEq(cfg.supplyCapE8s, 1_000_000_000_000);
        assertEq(cfg.mintWindowLimitE8s, 100_000_000_000);
        assertEq(cfg.mintWindowSeconds, 86400);
        assertEq(cfg.minBridgeOutE8s, 1_000_000);

        // Each flag is its own variable, and the Base flag is not one of them.
        script.setValue("ALLOW_ETHEREUM_MAINNET", "true");
        cfg = script.loadConfig();
        assertTrue(cfg.allowEthereumMainnet);
        assertFalse(cfg.allowArbitrumMainnet);
        assertFalse(cfg.allowMonadMainnet);

        script.setValue("ALLOW_ETHEREUM_MAINNET", "false");
        script.setValue("ALLOW_ARBITRUM_MAINNET", "true");
        cfg = script.loadConfig();
        assertFalse(cfg.allowEthereumMainnet);
        assertTrue(cfg.allowArbitrumMainnet);
        assertFalse(cfg.allowMonadMainnet);

        script.setValue("ALLOW_ARBITRUM_MAINNET", "false");
        script.setValue("ALLOW_MONAD_MAINNET", "true");
        cfg = script.loadConfig();
        assertFalse(cfg.allowEthereumMainnet);
        assertFalse(cfg.allowArbitrumMainnet);
        assertTrue(cfg.allowMonadMainnet);

        script.setValue("ALLOW_MONAD_MAINNET", "true");
        script.setValue("ALLOW_BASE_MAINNET", "true"); // the Base flag is not read here
        cfg = script.loadConfig();
        assertFalse(cfg.allowEthereumMainnet);
        assertFalse(cfg.allowArbitrumMainnet);

        // An explicit guardian is read.
        script.setValue("BRIDGE_GUARDIAN_ADDR", vm.toString(guardian));
        assertEq(script.loadConfig().guardian, guardian);
    }

    function test_env_loading_rejects_out_of_range_values() public {
        _fill("31337", vm.toString(uint256(type(uint48).max) + 1), "86400");
        vm.expectRevert("BRIDGE_ADMIN_DELAY_SECONDS exceeds uint48");
        script.loadConfig();

        _fill("31337", "172800", vm.toString(uint256(type(uint64).max) + 1));
        vm.expectRevert("BRIDGE_MINT_WINDOW_SECONDS exceeds uint64");
        script.loadConfig();

        // The largest accepted values load.
        _fill("31337", vm.toString(uint256(type(uint48).max)), vm.toString(uint256(type(uint64).max)));
        DeployBridgedIcUSDEvm.Config memory cfg = script.loadConfig();
        assertEq(cfg.adminDelaySeconds, type(uint48).max);
        assertEq(cfg.mintWindowSeconds, type(uint64).max);
    }

    function test_run_from_env_needs_the_mainnet_flag_of_the_chain() public {
        // run() reads the table, validates, then deploys on the in-memory EVM.
        _fill("31337", "172800", "86400");
        BridgedIcUSD t = script.run();
        assertEq(t.defaultAdmin(), admin);
        assertTrue(t.hasRole(t.MINTER_ROLE(), minter));
        assertFalse(t.hasRole(t.PAUSER_ROLE(), guardian));

        // Ethereum mainnet: refused with the flag down, deployed with only its own flag up.
        vm.chainId(ETHEREUM);
        _fill("1", "172800", "86400");
        vm.expectRevert("Ethereum mainnet (1) requires ALLOW_ETHEREUM_MAINNET=true");
        script.run();
        script.setValue("ALLOW_ARBITRUM_MAINNET", "true");
        script.setValue("ALLOW_MONAD_MAINNET", "true");
        vm.expectRevert("Ethereum mainnet (1) requires ALLOW_ETHEREUM_MAINNET=true");
        script.run();
        script.setValue("ALLOW_ETHEREUM_MAINNET", "true");
        t = script.run();
        assertEq(t.decimals(), 8);

        // The same table on Monad: the Monad flag decides, not the Ethereum one.
        vm.chainId(MONAD);
        _fill("143", "172800", "86400");
        script.setValue("ALLOW_MONAD_MAINNET", "false");
        vm.expectRevert("Monad mainnet (143) requires ALLOW_MONAD_MAINNET=true");
        script.run();
        script.setValue("ALLOW_MONAD_MAINNET", "true");
        t = script.run();
        assertEq(t.VERSION(), "rumi-base-icusd-bridge-v1");

        // Chain mismatch: EXPECTED_CHAIN_ID says Arbitrum One but we are on Monad.
        _fill("42161", "172800", "86400");
        vm.expectRevert("EXPECTED_CHAIN_ID does not match block.chainid");
        script.run();
    }
}
