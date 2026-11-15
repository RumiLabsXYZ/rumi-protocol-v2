// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {DeployBridgedIcUSD} from "../script/DeployBridgedIcUSD.s.sol";
import {BridgedIcUSD} from "../src/BridgedIcUSD.sol";

/// @dev Exposes the script's internal `deploy` to the tests. Everything here runs
/// against the in-memory Foundry EVM only: no RPC URL, no private key, nothing is
/// ever sent to a network (`vm.startBroadcast` in a test just records transactions).
contract DeployHarness is DeployBridgedIcUSD {
    function deployPublic(Config memory cfg) external returns (BridgedIcUSD) {
        return deploy(cfg);
    }
}

contract DeployBridgedIcUSDTest is Test {
    DeployHarness script;

    address admin = address(0xA11CE);
    address minter = address(0xB0B);
    address guardian = address(0x6A2D);

    function setUp() public {
        script = new DeployHarness();
        vm.chainId(31337);
    }

    function _cfg() internal view returns (DeployBridgedIcUSD.Config memory cfg) {
        cfg = DeployBridgedIcUSD.Config({
            expectedChainId: block.chainid,
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
    }

    // ------------------------------------------------------------------
    // Happy paths
    // ------------------------------------------------------------------

    function test_deploys_locally_with_intended_roles() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
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
        for (uint256 i = 0; i < deployers.length; i++) {
            assertFalse(t.hasRole(adminRole, deployers[i]));
            assertFalse(t.hasRole(minterRole, deployers[i]));
            assertFalse(t.hasRole(pauserRole, deployers[i]));
        }
    }

    function test_deploys_without_guardian() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.guardian = address(0);
        BridgedIcUSD t = script.deployPublic(cfg);
        assertFalse(t.hasRole(t.PAUSER_ROLE(), address(0)));
        assertTrue(t.hasRole(t.PAUSER_ROLE(), admin));
        assertTrue(t.hasRole(t.PAUSER_ROLE(), minter));
    }

    function test_base_sepolia_is_allowed() public {
        vm.chainId(84532);
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        BridgedIcUSD t = script.deployPublic(cfg);
        assertEq(t.decimals(), 8);
    }

    // ------------------------------------------------------------------
    // Chain guard
    // ------------------------------------------------------------------

    function test_rejects_expected_chain_id_mismatch() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.expectedChainId = 84532; // running on 31337
        vm.expectRevert("EXPECTED_CHAIN_ID does not match block.chainid");
        script.deployPublic(cfg);
    }

    function test_rejects_base_mainnet_without_allow_flag() public {
        vm.chainId(8453);
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.allowBaseMainnet = false;
        vm.expectRevert("Base mainnet (8453) requires ALLOW_BASE_MAINNET=true");
        script.deployPublic(cfg);
    }

    function test_allows_base_mainnet_only_with_flag() public {
        vm.chainId(8453);
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.allowBaseMainnet = true;
        // Validation passes; the deploy itself runs on the local in-memory EVM only.
        script.validateConfig(cfg);
        BridgedIcUSD t = script.deployPublic(cfg);
        assertEq(t.decimals(), 8);
    }

    function test_rejects_unknown_chain_ids() public {
        uint256[7] memory chains = [uint256(1), 10, 137, 8454, 84531, 10143, 1030];
        for (uint256 i = 0; i < chains.length; i++) {
            vm.chainId(chains[i]);
            DeployBridgedIcUSD.Config memory cfg = _cfg();
            cfg.allowBaseMainnet = true; // the flag must not unlock anything but 8453
            vm.expectRevert("unsupported chain id (allowed: 84532, 31337, and 8453 with ALLOW_BASE_MAINNET)");
            script.deployPublic(cfg);
        }
    }

    // ------------------------------------------------------------------
    // Role and parameter validation
    // ------------------------------------------------------------------

    function test_rejects_zero_admin() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.admin = address(0);
        vm.expectRevert("admin is the zero address");
        script.deployPublic(cfg);
    }

    function test_rejects_zero_minter() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.minter = address(0);
        vm.expectRevert("minter is the zero address");
        script.deployPublic(cfg);
    }

    function test_rejects_admin_equals_minter() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.minter = cfg.admin;
        vm.expectRevert("admin and minter must be different addresses");
        script.deployPublic(cfg);
    }

    function test_rejects_guardian_equal_to_admin_or_minter() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.guardian = cfg.admin;
        vm.expectRevert("guardian must be distinct from admin and minter");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.guardian = cfg.minter;
        vm.expectRevert("guardian must be distinct from admin and minter");
        script.deployPublic(cfg);
    }

    function test_rejects_zero_economic_parameters() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
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

    function test_rejects_deployer_that_is_a_role_holder() public {
        // msg.sender of the script call is this test contract.
        DeployBridgedIcUSD.Config memory cfg = _cfg();
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

        // tx.origin is checked too.
        (,, address origin) = vm.readCallers();
        cfg = _cfg();
        cfg.admin = origin;
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);

        cfg = _cfg();
        cfg.minter = origin;
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);
    }

    function test_rejects_broadcaster_that_is_a_role_holder() public {
        // The address that actually signs (`--account` / `--private-key` at run time).
        vm.startBroadcast();
        (, address broadcaster,) = vm.readCallers();
        vm.stopBroadcast();

        DeployBridgedIcUSD.Config memory cfg = _cfg();
        cfg.admin = broadcaster;
        vm.expectRevert("deployer must not be the admin, minter or guardian");
        script.deployPublic(cfg);
    }

    // ------------------------------------------------------------------
    // Post-deploy verification catches a wrongly configured token
    // ------------------------------------------------------------------

    function test_verify_deployment_detects_mismatches() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
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
        // Correct token passes.
        script.verifyDeployment(t, cfg, address(this));

        // Wrong expectations fail.
        DeployBridgedIcUSD.Config memory bad = _cfg();
        bad.supplyCapE8s = cfg.supplyCapE8s + 1;
        vm.expectRevert("supplyCap mismatch");
        script.verifyDeployment(t, bad, address(this));

        bad = _cfg();
        bad.minter = address(0xBAD);
        vm.expectRevert("minter lacks MINTER_ROLE");
        script.verifyDeployment(t, bad, address(this));

        bad = _cfg();
        bad.adminDelaySeconds = 1 days;
        vm.expectRevert("admin delay mismatch");
        script.verifyDeployment(t, bad, address(this));

        // A deployer that does hold a role is flagged (here: the admin pretending to be the deployer).
        vm.expectRevert("deployer holds DEFAULT_ADMIN_ROLE");
        script.verifyDeployment(t, cfg, admin);
        vm.expectRevert("deployer holds MINTER_ROLE");
        script.verifyDeployment(t, cfg, minter);
    }

    function test_verify_deployment_detects_unexpected_extra_roles() public {
        DeployBridgedIcUSD.Config memory cfg = _cfg();
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
        bytes32 minterRole = t.MINTER_ROLE();
        // Give the guardian MINTER_ROLE and confirm the check notices.
        vm.prank(admin);
        t.grantRole(minterRole, guardian);
        vm.expectRevert("guardian holds MINTER_ROLE");
        script.verifyDeployment(t, cfg, address(this));
    }

    // ------------------------------------------------------------------
    // Environment loading (single sequential test: env vars are process-global)
    // ------------------------------------------------------------------

    function _setEnv(
        uint256 chainId,
        string memory allowMainnet,
        address admin_,
        address minter_,
        string memory delay,
        string memory windowSeconds
    ) internal {
        vm.setEnv("EXPECTED_CHAIN_ID", vm.toString(chainId));
        vm.setEnv("ALLOW_BASE_MAINNET", allowMainnet);
        vm.setEnv("BRIDGE_ADMIN_ADDR", vm.toString(admin_));
        vm.setEnv("BRIDGE_MINTER_ADDR", vm.toString(minter_));
        vm.setEnv("BRIDGE_ADMIN_DELAY_SECONDS", delay);
        vm.setEnv("BRIDGE_SUPPLY_CAP_E8S", "1000000000000");
        vm.setEnv("BRIDGE_MINT_WINDOW_LIMIT_E8S", "100000000000");
        vm.setEnv("BRIDGE_MINT_WINDOW_SECONDS", windowSeconds);
        vm.setEnv("BRIDGE_MIN_BRIDGE_OUT_E8S", "1000000");
    }

    function test_env_loading_and_run() public {
        // (1) Required variables missing -> loadConfig reverts. Only meaningful when the
        // outer shell does not already export them, so skip this step otherwise.
        if (vm.envOr("EXPECTED_CHAIN_ID", uint256(0)) == 0 && vm.envOr("BRIDGE_ADMIN_ADDR", address(0)) == address(0)) {
            vm.expectRevert();
            script.loadConfig();
        }

        // (2) Guardian omitted -> defaults to address(0); mainnet flag defaults to false.
        if (vm.envOr("BRIDGE_GUARDIAN_ADDR", address(0)) == address(0)) {
            vm.setEnv("BRIDGE_GUARDIAN_ADDR", "");
        }
        _setEnv(31337, "false", admin, minter, "172800", "86400");
        DeployBridgedIcUSD.Config memory cfg = script.loadConfig();
        assertEq(cfg.expectedChainId, 31337);
        assertFalse(cfg.allowBaseMainnet);
        assertEq(cfg.admin, admin);
        assertEq(cfg.minter, minter);
        assertEq(cfg.guardian, address(0));
        assertEq(cfg.adminDelaySeconds, 172800);
        assertEq(cfg.supplyCapE8s, 1_000_000_000_000);
        assertEq(cfg.mintWindowLimitE8s, 100_000_000_000);
        assertEq(cfg.mintWindowSeconds, 86400);
        assertEq(cfg.minBridgeOutE8s, 1_000_000);

        // (3) Full run() on the local chain.
        BridgedIcUSD t = script.run();
        assertEq(t.defaultAdmin(), admin);
        assertTrue(t.hasRole(t.MINTER_ROLE(), minter));
        assertFalse(t.hasRole(t.PAUSER_ROLE(), guardian));

        // (4) Explicit guardian.
        vm.setEnv("BRIDGE_GUARDIAN_ADDR", vm.toString(guardian));
        t = script.run();
        assertTrue(t.hasRole(t.PAUSER_ROLE(), guardian));
        assertFalse(t.hasRole(t.MINTER_ROLE(), guardian));

        // (5) Chain mismatch: EXPECTED_CHAIN_ID says Sepolia but we are on 31337.
        _setEnv(84532, "false", admin, minter, "172800", "86400");
        vm.expectRevert("EXPECTED_CHAIN_ID does not match block.chainid");
        script.run();

        // (6) Base mainnet needs the flag, even when EXPECTED_CHAIN_ID matches.
        vm.chainId(8453);
        _setEnv(8453, "false", admin, minter, "172800", "86400");
        vm.expectRevert("Base mainnet (8453) requires ALLOW_BASE_MAINNET=true");
        script.run();
        _setEnv(8453, "true", admin, minter, "172800", "86400");
        t = script.run(); // local in-memory EVM only; nothing is sent anywhere
        assertEq(t.decimals(), 8);

        // (7) Unknown chain id is rejected even when it matches EXPECTED_CHAIN_ID and the flag is set.
        vm.chainId(1);
        _setEnv(1, "true", admin, minter, "172800", "86400");
        vm.expectRevert("unsupported chain id (allowed: 84532, 31337, and 8453 with ALLOW_BASE_MAINNET)");
        script.run();
        vm.chainId(31337);

        // (8) Out-of-range numeric env values are rejected, not silently truncated.
        _setEnv(31337, "false", admin, minter, vm.toString(uint256(type(uint48).max) + 1), "86400");
        vm.expectRevert("BRIDGE_ADMIN_DELAY_SECONDS exceeds uint48");
        script.loadConfig();
        _setEnv(31337, "false", admin, minter, "172800", vm.toString(uint256(type(uint64).max) + 1));
        vm.expectRevert("BRIDGE_MINT_WINDOW_SECONDS exceeds uint64");
        script.loadConfig();

        // (9) Garbage in a required numeric variable reverts.
        vm.setEnv("EXPECTED_CHAIN_ID", "not-a-number");
        vm.expectRevert();
        script.loadConfig();
    }
}
