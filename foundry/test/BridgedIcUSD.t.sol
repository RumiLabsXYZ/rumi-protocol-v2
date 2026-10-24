// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {StdInvariant} from "forge-std/StdInvariant.sol";
import {Vm} from "forge-std/Vm.sol";
import {BridgedIcUSD} from "../src/BridgedIcUSD.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {
    IAccessControlDefaultAdminRules
} from "@openzeppelin/contracts/access/extensions/IAccessControlDefaultAdminRules.sol";
import {IERC20Errors} from "@openzeppelin/contracts/interfaces/draft-IERC6093.sol";

contract BridgedIcUSDTest is Test {
    BridgedIcUSD token;

    address admin = address(0xA11CE);
    address minter = address(0xB0B);
    address guardian = address(0x6A2D);
    address alice = address(0xCAFE);
    address bob = address(0xBEEF);
    address carol = address(0xCA201);

    uint48 constant ADMIN_DELAY = 2 days;
    uint256 constant SUPPLY_CAP = 1_000_000e8; // 1M icUSD
    uint256 constant WINDOW_LIMIT = 100_000e8; // 100k icUSD per window
    uint64 constant WINDOW_SECONDS = 1 days;
    uint256 constant MIN_BRIDGE_OUT = 1e6; // 0.01 icUSD

    // Role ids cached so a one-shot vm.prank is never consumed by a getter call.
    bytes32 ADMIN_ROLE;
    bytes32 MINTER;
    bytes32 PAUSER;

    // A 10-byte canister-style ICP principal and a nonzero subaccount.
    bytes constant PRINCIPAL = hex"00000000000000020101";
    bytes32 constant SUBACCOUNT = bytes32(uint256(0xabc));
    bytes32 constant NONCE = bytes32(uint256(1));

    // Re-declared for vm.expectEmit (must match BridgedIcUSD exactly, including `indexed`).
    event BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount);
    event BridgeOut(
        bytes32 indexed transferId,
        address indexed from,
        uint256 amount,
        uint64 sequence,
        bytes icpPrincipal,
        bytes32 icpSubaccount
    );
    event MintPausedSet(bool paused);
    event BridgeOutPausedSet(bool paused);
    event SupplyCapSet(uint256 cap);
    event MintRateLimitSet(uint256 limit, uint64 windowSeconds);
    event MinBridgeOutSet(uint256 minimum);
    event Transfer(address indexed from, address indexed to, uint256 value);

    function setUp() public {
        token = _deploy();
        ADMIN_ROLE = token.DEFAULT_ADMIN_ROLE();
        MINTER = token.MINTER_ROLE();
        PAUSER = token.PAUSER_ROLE();
    }

    function _deploy() internal returns (BridgedIcUSD) {
        return new BridgedIcUSD(
            admin, ADMIN_DELAY, minter, guardian, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT
        );
    }

    function _mint(address to, uint256 amount, bytes32 id) internal {
        vm.prank(minter);
        token.mint(to, amount, id);
    }

    function _bridgeOut(address from, uint256 amount, bytes32 nonce) internal returns (bytes32 id) {
        vm.prank(from);
        id = token.bridgeOut(amount, PRINCIPAL, SUBACCOUNT, nonce);
    }

    function _unauthorized(address account, bytes32 role) internal pure returns (bytes memory) {
        return abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, account, role);
    }

    // ------------------------------------------------------------------
    // Basics and constructor
    // ------------------------------------------------------------------

    function test_decimals_is_8() public view {
        assertEq(token.decimals(), 8);
    }

    function test_metadata_and_constants() public view {
        assertEq(token.name(), "Rumi icUSD");
        assertEq(token.symbol(), "icUSD");
        assertEq(token.VERSION(), "rumi-base-icusd-bridge-v1");
        assertEq(token.ICP_PRINCIPAL_MAX_LEN(), 29);
        assertEq(token.MINTER_ROLE(), keccak256("MINTER_ROLE"));
        assertEq(token.PAUSER_ROLE(), keccak256("PAUSER_ROLE"));
        assertEq(token.BRIDGE_OUT_DOMAIN(), keccak256("rumi.icusd.base.bridge-out.v1"));
        assertEq(token.totalSupply(), 0);
    }

    function test_constructor_config_and_roles() public view {
        // Config.
        assertEq(token.supplyCap(), SUPPLY_CAP);
        assertEq(token.mintWindowLimit(), WINDOW_LIMIT);
        assertEq(token.mintWindowSeconds(), WINDOW_SECONDS);
        assertEq(token.mintWindowStart(), uint64(block.timestamp));
        assertEq(token.mintedInWindow(), 0);
        assertEq(token.minBridgeOut(), MIN_BRIDGE_OUT);
        assertFalse(token.mintPaused());
        assertFalse(token.bridgeOutPaused());
        assertEq(token.bridgeOutCount(), 0);
        assertEq(token.defaultAdmin(), admin);
        assertEq(token.owner(), admin);
        assertEq(token.defaultAdminDelay(), ADMIN_DELAY);

        // Admin: DEFAULT_ADMIN + PAUSER, never MINTER.
        assertTrue(token.hasRole(ADMIN_ROLE, admin));
        assertTrue(token.hasRole(PAUSER, admin));
        assertFalse(token.hasRole(MINTER, admin));

        // Minter: MINTER + PAUSER only.
        assertTrue(token.hasRole(MINTER, minter));
        assertTrue(token.hasRole(PAUSER, minter));
        assertFalse(token.hasRole(ADMIN_ROLE, minter));

        // Guardian: pause-only.
        assertTrue(token.hasRole(PAUSER, guardian));
        assertFalse(token.hasRole(MINTER, guardian));
        assertFalse(token.hasRole(ADMIN_ROLE, guardian));

        // Deployer (this test contract) keeps nothing.
        assertFalse(token.hasRole(ADMIN_ROLE, address(this)));
        assertFalse(token.hasRole(MINTER, address(this)));
        assertFalse(token.hasRole(PAUSER, address(this)));

        // Bystanders hold nothing.
        assertFalse(token.hasRole(ADMIN_ROLE, alice));
        assertFalse(token.hasRole(MINTER, alice));
        assertFalse(token.hasRole(PAUSER, alice));
    }

    function test_constructor_without_guardian_grants_nothing_extra() public {
        BridgedIcUSD t = new BridgedIcUSD(
            admin, ADMIN_DELAY, minter, address(0), SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT
        );
        assertFalse(t.hasRole(PAUSER, address(0)));
        assertFalse(t.hasRole(PAUSER, guardian));
        assertTrue(t.hasRole(PAUSER, admin));
        assertTrue(t.hasRole(PAUSER, minter));
        assertFalse(t.hasRole(t.DEFAULT_ADMIN_ROLE(), address(this)));
    }

    function test_constructor_emits_config_events() public {
        address predicted = vm.computeCreateAddress(address(this), vm.getNonce(address(this)));
        vm.expectEmit(true, true, true, true, predicted);
        emit SupplyCapSet(SUPPLY_CAP);
        vm.expectEmit(true, true, true, true, predicted);
        emit MintRateLimitSet(WINDOW_LIMIT, WINDOW_SECONDS);
        vm.expectEmit(true, true, true, true, predicted);
        emit MinBridgeOutSet(MIN_BRIDGE_OUT);
        BridgedIcUSD t = _deploy();
        assertEq(address(t), predicted);
    }

    function test_constructor_rejects_zero_admin() public {
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(
            address(0), ADMIN_DELAY, minter, guardian, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT
        );
    }

    function test_constructor_rejects_zero_minter() public {
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(
            admin, ADMIN_DELAY, address(0), guardian, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT
        );
    }

    function test_constructor_rejects_admin_equals_minter() public {
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(admin, ADMIN_DELAY, admin, guardian, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT);
    }

    function test_constructor_rejects_zero_window() public {
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(admin, ADMIN_DELAY, minter, guardian, SUPPLY_CAP, WINDOW_LIMIT, 0, MIN_BRIDGE_OUT);
    }

    function test_constructor_rejects_guardian_with_other_role() public {
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(admin, ADMIN_DELAY, minter, admin, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT);
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        new BridgedIcUSD(admin, ADMIN_DELAY, minter, minter, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT);
    }

    function test_deployer_as_admin_is_the_only_way_to_hold_a_role() public {
        // If the deployer is explicitly passed as admin it holds DEFAULT_ADMIN + PAUSER and nothing else.
        BridgedIcUSD t = new BridgedIcUSD(
            address(this), 0, minter, address(0), SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT
        );
        assertTrue(t.hasRole(t.DEFAULT_ADMIN_ROLE(), address(this)));
        assertTrue(t.hasRole(PAUSER, address(this)));
        assertFalse(t.hasRole(MINTER, address(this)));
    }

    // ------------------------------------------------------------------
    // mint
    // ------------------------------------------------------------------

    function test_minter_mints_and_emits_events() public {
        uint256 amount = 10_000_000_000;
        bytes32 id = keccak256("mint-1");
        vm.expectEmit(true, true, false, true, address(token));
        emit Transfer(address(0), alice, amount);
        vm.expectEmit(true, true, false, true, address(token));
        emit BridgeMint(id, alice, amount);
        _mint(alice, amount, id);
        assertEq(token.balanceOf(alice), amount);
        assertEq(token.totalSupply(), amount);
        assertTrue(token.mintConsumed(id));
        assertEq(token.mintedInWindow(), amount);
    }

    function test_non_minter_cannot_mint() public {
        address[4] memory callers = [alice, admin, guardian, address(this)];
        for (uint256 i = 0; i < callers.length; i++) {
            address c = callers[i];
            vm.prank(c);
            vm.expectRevert(_unauthorized(c, MINTER));
            token.mint(alice, 1, bytes32(i + 1));
        }
        assertEq(token.totalSupply(), 0);
    }

    function test_duplicate_mint_id_reverts_and_leaves_supply_unchanged() public {
        bytes32 id = keccak256("dup");
        _mint(alice, 500, id);
        uint256 supplyBefore = token.totalSupply();
        uint256 windowBefore = token.mintedInWindow();

        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintAlreadyConsumed.selector, id));
        token.mint(alice, 500, id);

        // Same id with a different recipient and amount is also rejected.
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintAlreadyConsumed.selector, id));
        token.mint(bob, 7, id);

        assertEq(token.totalSupply(), supplyBefore);
        assertEq(token.mintedInWindow(), windowBefore);
        assertEq(token.balanceOf(bob), 0);
    }

    function test_mint_to_zero_or_self_reverts() public {
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.InvalidRecipient.selector, address(0)));
        token.mint(address(0), 1, bytes32(uint256(1)));

        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.InvalidRecipient.selector, address(token)));
        token.mint(address(token), 1, bytes32(uint256(2)));

        // The id was not consumed by the failed attempts.
        assertFalse(token.mintConsumed(bytes32(uint256(1))));
        assertFalse(token.mintConsumed(bytes32(uint256(2))));
    }

    function test_mint_zero_amount_reverts() public {
        vm.prank(minter);
        vm.expectRevert(BridgedIcUSD.ZeroAmount.selector);
        token.mint(alice, 0, bytes32(uint256(1)));
    }

    function test_mint_amount_above_uint64_max_reverts() public {
        uint256 tooBig = uint256(type(uint64).max) + 1;
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.AmountTooLarge.selector, tooBig));
        token.mint(alice, tooBig, bytes32(uint256(1)));
    }

    function test_mint_amount_uint64_max_is_accepted_with_room() public {
        BridgedIcUSD t = new BridgedIcUSD(
            admin, ADMIN_DELAY, minter, guardian, type(uint256).max, type(uint256).max, 1 days, MIN_BRIDGE_OUT
        );
        vm.prank(minter);
        t.mint(alice, type(uint64).max, bytes32(uint256(1)));
        assertEq(t.balanceOf(alice), type(uint64).max);
    }

    function test_supply_cap_boundary() public {
        // Lift the rate limit out of the way so only the cap is under test.
        vm.prank(admin);
        token.setMintRateLimit(type(uint256).max, WINDOW_SECONDS);
        vm.prank(admin);
        token.setSupplyCap(1_000);

        _mint(alice, 1_000, bytes32(uint256(1))); // exactly at the cap
        assertEq(token.totalSupply(), 1_000);

        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.SupplyCapExceeded.selector, 1_001, 1_000));
        token.mint(alice, 1, bytes32(uint256(2)));
        assertFalse(token.mintConsumed(bytes32(uint256(2))));

        // Burning frees room again.
        _bridgeOutAmount(alice, 400, bytes32(uint256(77)));
        _mint(alice, 400, bytes32(uint256(3)));
        assertEq(token.totalSupply(), 1_000);
    }

    function _bridgeOutAmount(address from, uint256 amount, bytes32 nonce) internal {
        vm.prank(admin);
        token.setMinBridgeOut(1);
        vm.prank(from);
        token.bridgeOut(amount, PRINCIPAL, SUBACCOUNT, nonce);
    }

    function test_supply_cap_one_below_and_partial_fill() public {
        vm.prank(admin);
        token.setMintRateLimit(type(uint256).max, WINDOW_SECONDS);
        vm.prank(admin);
        token.setSupplyCap(1_000);
        _mint(alice, 999, bytes32(uint256(1)));
        _mint(alice, 1, bytes32(uint256(2)));
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.SupplyCapExceeded.selector, 1_001, 1_000));
        token.mint(alice, 1, bytes32(uint256(3)));
    }

    function test_lowering_supply_cap_below_supply_blocks_mint_but_not_bridge_out() public {
        _mint(alice, 5_000_000, bytes32(uint256(1)));
        vm.prank(admin);
        token.setSupplyCap(1_000_000); // below totalSupply: allowed
        assertEq(token.supplyCap(), 1_000_000);
        assertEq(token.totalSupply(), 5_000_000);

        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.SupplyCapExceeded.selector, 5_000_001, 1_000_000));
        token.mint(alice, 1, bytes32(uint256(2)));

        // Existing holders can still bridge out.
        _bridgeOut(alice, 2_000_000, NONCE);
        assertEq(token.totalSupply(), 3_000_000);
    }

    function test_rate_limit_boundary_within_window() public {
        _mint(alice, WINDOW_LIMIT - 1, bytes32(uint256(1)));
        assertEq(token.mintedInWindow(), WINDOW_LIMIT - 1);

        // One more base unit fits exactly.
        _mint(alice, 1, bytes32(uint256(2)));
        assertEq(token.mintedInWindow(), WINDOW_LIMIT);

        // One more does not (remaining == 0).
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, 1, 0));
        token.mint(alice, 1, bytes32(uint256(3)));
        assertFalse(token.mintConsumed(bytes32(uint256(3))));
        assertEq(token.mintedInWindow(), WINDOW_LIMIT);
    }

    function test_rate_limit_reports_remaining() public {
        _mint(alice, WINDOW_LIMIT - 10, bytes32(uint256(1)));
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, 11, 10));
        token.mint(alice, 11, bytes32(uint256(2)));
    }

    function test_rate_limit_resets_after_window_elapses() public {
        uint64 start = token.mintWindowStart();
        _mint(alice, WINDOW_LIMIT, bytes32(uint256(1)));

        // One second before the window ends: still blocked, window unchanged.
        vm.warp(uint256(start) + WINDOW_SECONDS - 1);
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, 1, 0));
        token.mint(alice, 1, bytes32(uint256(2)));

        // At start + window the window rolls over.
        vm.warp(uint256(start) + WINDOW_SECONDS);
        _mint(alice, WINDOW_LIMIT, bytes32(uint256(3)));
        // forge-lint: disable-next-line(unsafe-typecast)
        assertEq(token.mintWindowStart(), uint64(start + WINDOW_SECONDS));
        assertEq(token.mintedInWindow(), WINDOW_LIMIT);
        assertEq(token.totalSupply(), 2 * WINDOW_LIMIT);

        // A far-future mint opens a fresh window starting now.
        vm.warp(block.timestamp + 10 * uint256(WINDOW_SECONDS));
        _mint(bob, 5, bytes32(uint256(4)));
        assertEq(token.mintWindowStart(), uint64(block.timestamp));
        assertEq(token.mintedInWindow(), 5);
    }

    function test_failed_mint_after_window_expiry_does_not_reset_window() public {
        uint64 start = token.mintWindowStart();
        _mint(alice, 10, bytes32(uint256(1)));
        vm.warp(uint256(start) + WINDOW_SECONDS + 5);
        // Amount above the limit reverts; the whole call (including the window reset) is undone.
        vm.prank(minter);
        vm.expectRevert(
            abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, WINDOW_LIMIT + 1, WINDOW_LIMIT)
        );
        token.mint(alice, WINDOW_LIMIT + 1, bytes32(uint256(2)));
        assertEq(token.mintWindowStart(), start);
        assertEq(token.mintedInWindow(), 10);
    }

    function test_lowering_rate_limit_below_minted_blocks_until_window_ends() public {
        _mint(alice, 1_000, bytes32(uint256(1)));
        vm.prank(admin);
        token.setMintRateLimit(500, WINDOW_SECONDS);
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, 1, 0));
        token.mint(alice, 1, bytes32(uint256(2)));

        vm.warp(uint256(token.mintWindowStart()) + WINDOW_SECONDS);
        _mint(alice, 500, bytes32(uint256(3)));
        assertEq(token.mintedInWindow(), 500);
    }

    function test_huge_window_does_not_overflow_or_trap() public {
        vm.prank(admin);
        token.setMintRateLimit(WINDOW_LIMIT, type(uint64).max);
        _mint(alice, 1, bytes32(uint256(1)));
        vm.warp(block.timestamp + 365 days);
        // Window is effectively never-ending; still no panic, limit still enforced.
        _mint(alice, WINDOW_LIMIT - 1, bytes32(uint256(2)));
        vm.prank(minter);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, 1, 0));
        token.mint(alice, 1, bytes32(uint256(3)));
    }

    // ------------------------------------------------------------------
    // Pause controls
    // ------------------------------------------------------------------

    function test_pause_mint_blocks_mint_and_only_admin_can_unpause() public {
        vm.prank(guardian);
        vm.expectEmit(false, false, false, true, address(token));
        emit MintPausedSet(true);
        token.pauseMint();
        assertTrue(token.mintPaused());

        vm.prank(minter);
        vm.expectRevert(BridgedIcUSD.MintIsPaused.selector);
        token.mint(alice, 1, bytes32(uint256(1)));

        // Guardian and minter can pause but cannot unpause.
        vm.prank(guardian);
        vm.expectRevert(_unauthorized(guardian, ADMIN_ROLE));
        token.unpauseMint();
        vm.prank(minter);
        vm.expectRevert(_unauthorized(minter, ADMIN_ROLE));
        token.unpauseMint();
        vm.prank(alice);
        vm.expectRevert(_unauthorized(alice, ADMIN_ROLE));
        token.unpauseMint();
        assertTrue(token.mintPaused());

        vm.prank(admin);
        vm.expectEmit(false, false, false, true, address(token));
        emit MintPausedSet(false);
        token.unpauseMint();
        assertFalse(token.mintPaused());
        _mint(alice, 1, bytes32(uint256(1)));
    }

    function test_minter_and_admin_can_pause_mint() public {
        vm.prank(minter);
        token.pauseMint();
        assertTrue(token.mintPaused());
        vm.prank(admin);
        token.unpauseMint();
        vm.prank(admin);
        token.pauseMint();
        assertTrue(token.mintPaused());
    }

    function test_non_pauser_cannot_pause() public {
        vm.prank(alice);
        vm.expectRevert(_unauthorized(alice, PAUSER));
        token.pauseMint();
        vm.prank(alice);
        vm.expectRevert(_unauthorized(alice, PAUSER));
        token.pauseBridgeOut();
    }

    function test_pause_bridge_out_blocks_bridge_out_but_not_transfers() public {
        _mint(alice, 10_000_000, bytes32(uint256(1)));

        vm.prank(guardian);
        vm.expectEmit(false, false, false, true, address(token));
        emit BridgeOutPausedSet(true);
        token.pauseBridgeOut();
        assertTrue(token.bridgeOutPaused());

        vm.prank(alice);
        vm.expectRevert(BridgedIcUSD.BridgeOutIsPaused.selector);
        token.bridgeOut(MIN_BRIDGE_OUT, PRINCIPAL, SUBACCOUNT, NONCE);

        // Plain ERC-20 movement is unaffected.
        vm.prank(alice);
        assertTrue(token.transfer(bob, 4_000_000));
        vm.prank(alice);
        assertTrue(token.approve(bob, 1_000_000));
        vm.prank(bob);
        assertTrue(token.transferFrom(alice, carol, 1_000_000));
        assertEq(token.balanceOf(alice), 5_000_000);
        assertEq(token.balanceOf(bob), 4_000_000);
        assertEq(token.balanceOf(carol), 1_000_000);

        // Guardian cannot unpause; only admin.
        vm.prank(guardian);
        vm.expectRevert(_unauthorized(guardian, ADMIN_ROLE));
        token.unpauseBridgeOut();
        vm.prank(admin);
        vm.expectEmit(false, false, false, true, address(token));
        emit BridgeOutPausedSet(false);
        token.unpauseBridgeOut();
        _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE);
    }

    function test_mint_pause_does_not_block_bridge_out_and_vice_versa() public {
        _mint(alice, 10_000_000, bytes32(uint256(1)));
        vm.prank(guardian);
        token.pauseMint();
        _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE); // still works

        vm.prank(admin);
        token.unpauseMint();
        vm.prank(guardian);
        token.pauseBridgeOut();
        _mint(alice, 1, bytes32(uint256(2))); // still works
    }

    // ------------------------------------------------------------------
    // bridgeOut
    // ------------------------------------------------------------------

    function test_bridge_out_happy_path() public {
        _mint(alice, 10_000_000_000, bytes32(uint256(1)));
        uint256 amount = 2_500_000_000;
        bytes32 expectedId = token.computeTransferId(alice, NONCE);

        vm.expectEmit(true, true, true, true, address(token));
        emit Transfer(alice, address(0), amount);
        vm.expectEmit(true, true, false, true, address(token));
        emit BridgeOut(expectedId, alice, amount, 1, PRINCIPAL, SUBACCOUNT);
        bytes32 id = _bridgeOut(alice, amount, NONCE);

        assertEq(id, expectedId);
        assertEq(token.balanceOf(alice), 10_000_000_000 - amount);
        assertEq(token.totalSupply(), 10_000_000_000 - amount);
        assertEq(token.bridgeOutCount(), 1);
        assertTrue(token.bridgeOutUsed(id));

        // Independent recomputation of the id.
        bytes32 independent = keccak256(
            abi.encode(keccak256("rumi.icusd.base.bridge-out.v1"), block.chainid, address(token), alice, NONCE)
        );
        assertEq(id, independent);
    }

    function test_bridge_out_sequence_increments_per_call() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        for (uint256 i = 1; i <= 3; i++) {
            bytes32 nonce = bytes32(i);
            bytes32 expectedId = token.computeTransferId(alice, nonce);
            vm.expectEmit(true, true, false, true, address(token));
            // forge-lint: disable-next-line(unsafe-typecast)
            emit BridgeOut(expectedId, alice, MIN_BRIDGE_OUT, uint64(i), PRINCIPAL, SUBACCOUNT);
            _bridgeOut(alice, MIN_BRIDGE_OUT, nonce);
            // forge-lint: disable-next-line(unsafe-typecast)
            assertEq(token.bridgeOutCount(), uint64(i));
        }
    }

    function test_same_sender_same_nonce_reverts() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        bytes32 id = _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE);
        uint256 supply = token.totalSupply();

        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.BridgeOutAlreadyUsed.selector, id));
        token.bridgeOut(MIN_BRIDGE_OUT, PRINCIPAL, SUBACCOUNT, NONCE);

        // Different destination or amount does not matter: the (sender, nonce) pair is spent.
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.BridgeOutAlreadyUsed.selector, id));
        token.bridgeOut(MIN_BRIDGE_OUT * 2, hex"01", bytes32(0), NONCE);

        assertEq(token.totalSupply(), supply);
        assertEq(token.bridgeOutCount(), 1);
    }

    function test_different_senders_same_nonce_get_distinct_ids() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        _mint(bob, 100_000_000, bytes32(uint256(2)));
        bytes32 idA = _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE);
        bytes32 idB = _bridgeOut(bob, MIN_BRIDGE_OUT, NONCE);
        assertTrue(idA != idB);
        assertEq(token.bridgeOutCount(), 2);
    }

    function test_transfer_id_depends_on_chain_and_contract() public {
        bytes32 id = token.computeTransferId(alice, NONCE);

        BridgedIcUSD other = _deploy();
        assertTrue(other.computeTransferId(alice, NONCE) != id);

        vm.chainId(block.chainid + 1);
        assertTrue(token.computeTransferId(alice, NONCE) != id);
    }

    function test_transfer_id_pinned_vector() public {
        // Cross-language vector for the Rust decoder and TS encoder: the token is
        // placed at a fixed address, chain id 8453 (Base mainnet), sender 0xA11CE, nonce 1.
        address fixedAddr = address(uint160(0xB45E1));
        deployCodeTo(
            "BridgedIcUSD.sol:BridgedIcUSD",
            abi.encode(admin, ADMIN_DELAY, minter, guardian, SUPPLY_CAP, WINDOW_LIMIT, WINDOW_SECONDS, MIN_BRIDGE_OUT),
            fixedAddr
        );
        vm.chainId(8453);
        bytes32 id = BridgedIcUSD(fixedAddr).computeTransferId(address(0xA11CE), bytes32(uint256(1)));
        assertEq(id, 0xf2f74fbfe0e8d729020d775d796ee6ff86aa29273d4873986667fde1c136336c);
    }

    function test_bridge_out_below_minimum_reverts() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(BridgedIcUSD.AmountBelowMinimum.selector, MIN_BRIDGE_OUT - 1, MIN_BRIDGE_OUT)
        );
        token.bridgeOut(MIN_BRIDGE_OUT - 1, PRINCIPAL, SUBACCOUNT, NONCE);

        // Exactly the minimum is fine.
        _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE);
    }

    function test_bridge_out_zero_reverts_even_with_zero_minimum() public {
        _mint(alice, 100, bytes32(uint256(1)));
        vm.prank(admin);
        token.setMinBridgeOut(0);
        vm.prank(alice);
        vm.expectRevert(BridgedIcUSD.ZeroAmount.selector);
        token.bridgeOut(0, PRINCIPAL, SUBACCOUNT, NONCE);
    }

    function test_bridge_out_above_uint64_max_reverts() public {
        uint256 tooBig = uint256(type(uint64).max) + 1;
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.AmountTooLarge.selector, tooBig));
        token.bridgeOut(tooBig, PRINCIPAL, SUBACCOUNT, NONCE);
    }

    function test_bridge_out_insufficient_balance_reverts() public {
        _mint(alice, MIN_BRIDGE_OUT, bytes32(uint256(1)));
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(
                IERC20Errors.ERC20InsufficientBalance.selector, alice, MIN_BRIDGE_OUT, MIN_BRIDGE_OUT + 1
            )
        );
        token.bridgeOut(MIN_BRIDGE_OUT + 1, PRINCIPAL, SUBACCOUNT, NONCE);

        // A failed burn must not consume the nonce or bump the counter.
        assertFalse(token.bridgeOutUsed(token.computeTransferId(alice, NONCE)));
        assertEq(token.bridgeOutCount(), 0);
        _bridgeOut(alice, MIN_BRIDGE_OUT, NONCE);
    }

    function test_bridge_out_rejects_bad_principals() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));

        // Empty (management canister principal).
        vm.prank(alice);
        vm.expectRevert(BridgedIcUSD.InvalidIcpPrincipal.selector);
        token.bridgeOut(MIN_BRIDGE_OUT, hex"", SUBACCOUNT, NONCE);

        // 30 bytes.
        bytes memory tooLong = new bytes(30);
        tooLong[0] = 0x01;
        vm.prank(alice);
        vm.expectRevert(BridgedIcUSD.InvalidIcpPrincipal.selector);
        token.bridgeOut(MIN_BRIDGE_OUT, tooLong, SUBACCOUNT, NONCE);

        // Anonymous principal.
        vm.prank(alice);
        vm.expectRevert(BridgedIcUSD.InvalidIcpPrincipal.selector);
        token.bridgeOut(MIN_BRIDGE_OUT, hex"04", SUBACCOUNT, NONCE);

        assertEq(token.bridgeOutCount(), 0);
        assertEq(token.balanceOf(alice), 100_000_000);
    }

    function test_bridge_out_accepts_boundary_principals() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));

        // 29 bytes is the maximum.
        bytes memory max29 = new bytes(29);
        for (uint256 i = 0; i < 29; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            max29[i] = bytes1(uint8(i + 1));
        }
        vm.prank(alice);
        token.bridgeOut(MIN_BRIDGE_OUT, max29, SUBACCOUNT, bytes32(uint256(1)));

        // 1-byte principals other than the anonymous one are structurally acceptable.
        vm.prank(alice);
        token.bridgeOut(MIN_BRIDGE_OUT, hex"05", SUBACCOUNT, bytes32(uint256(2)));

        // A longer principal that merely starts with 0x04 is not the anonymous principal.
        vm.prank(alice);
        token.bridgeOut(MIN_BRIDGE_OUT, hex"0400", SUBACCOUNT, bytes32(uint256(3)));

        assertEq(token.bridgeOutCount(), 3);
    }

    function test_bridge_out_accepts_zero_subaccount() public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        vm.prank(alice);
        token.bridgeOut(MIN_BRIDGE_OUT, PRINCIPAL, bytes32(0), NONCE);
        assertEq(token.bridgeOutCount(), 1);
    }

    // ------------------------------------------------------------------
    // No other mint or burn path
    // ------------------------------------------------------------------

    function test_no_repayment_burn_or_other_burn_selectors() public {
        _mint(alice, 1_000, bytes32(uint256(1)));
        bytes[5] memory calls = [
            abi.encodeWithSelector(bytes4(keccak256("burn(uint256,uint64)")), uint256(1), uint64(1)),
            abi.encodeWithSelector(bytes4(keccak256("burn(uint256)")), uint256(1)),
            abi.encodeWithSelector(bytes4(keccak256("burnFrom(address,uint256)")), alice, uint256(1)),
            abi.encodeWithSelector(bytes4(keccak256("mint(address,uint256)")), alice, uint256(1)),
            abi.encodeWithSelector(
                bytes4(keccak256("mint(address,uint256,uint64,uint64)")), alice, uint256(1), uint64(1), uint64(1)
            )
        ];
        for (uint256 i = 0; i < calls.length; i++) {
            vm.prank(i == 3 || i == 4 ? minter : alice);
            (bool ok,) = address(token).call(calls[i]);
            assertFalse(ok, "unexpected selector accepted");
        }
        assertEq(token.balanceOf(alice), 1_000);
        assertEq(token.totalSupply(), 1_000);
    }

    function test_no_fallback_or_receive() public {
        (bool ok,) = address(token).call("");
        assertFalse(ok);
        (ok,) = address(token).call(hex"deadbeef");
        assertFalse(ok);
        vm.deal(address(this), 1 ether);
        (ok,) = address(token).call{value: 1}("");
        assertFalse(ok);
    }

    function test_abi_pin_no_burn_selector_in_runtime_dispatch() public view {
        // The 4-byte selectors of the forbidden entry points must not appear as
        // PUSH4 operands in the runtime code, which is how solc dispatches.
        bytes memory code = address(token).code;
        bytes4[4] memory forbidden = [
            bytes4(0x3ebfd46f), // burn(uint256,uint64)
            bytes4(0x42966c68), // burn(uint256)
            bytes4(keccak256("burnFrom(address,uint256)")),
            bytes4(keccak256("mint(address,uint256,uint64,uint64)"))
        ];
        for (uint256 f = 0; f < forbidden.length; f++) {
            assertFalse(_containsPush4(code, forbidden[f]), "forbidden selector present in runtime code");
        }
        // Sanity: the scan does find a selector that is present (bridgeOut).
        assertTrue(_containsPush4(code, 0x803451ee));
    }

    function _containsPush4(bytes memory code, bytes4 sel) internal pure returns (bool) {
        if (code.length < 5) return false;
        for (uint256 i = 0; i + 4 < code.length; i++) {
            if (
                code[i] == 0x63 && code[i + 1] == sel[0] && code[i + 2] == sel[1] && code[i + 3] == sel[2]
                    && code[i + 4] == sel[3]
            ) {
                return true;
            }
        }
        return false;
    }

    // ------------------------------------------------------------------
    // Admin configuration and roles
    // ------------------------------------------------------------------

    function test_admin_setters_emit_events_and_update_state() public {
        vm.startPrank(admin);

        vm.expectEmit(false, false, false, true, address(token));
        emit SupplyCapSet(42);
        token.setSupplyCap(42);
        assertEq(token.supplyCap(), 42);

        vm.expectEmit(false, false, false, true, address(token));
        emit MintRateLimitSet(7, 3600);
        token.setMintRateLimit(7, 3600);
        assertEq(token.mintWindowLimit(), 7);
        assertEq(token.mintWindowSeconds(), 3600);

        vm.expectEmit(false, false, false, true, address(token));
        emit MinBridgeOutSet(9);
        token.setMinBridgeOut(9);
        assertEq(token.minBridgeOut(), 9);

        vm.stopPrank();
    }

    function test_setters_reject_non_admin() public {
        address[3] memory callers = [minter, guardian, alice];
        for (uint256 i = 0; i < callers.length; i++) {
            address c = callers[i];
            vm.prank(c);
            vm.expectRevert(_unauthorized(c, ADMIN_ROLE));
            token.setSupplyCap(1);
            vm.prank(c);
            vm.expectRevert(_unauthorized(c, ADMIN_ROLE));
            token.setMintRateLimit(1, 1);
            vm.prank(c);
            vm.expectRevert(_unauthorized(c, ADMIN_ROLE));
            token.setMinBridgeOut(1);
        }
        assertEq(token.supplyCap(), SUPPLY_CAP);
    }

    function test_set_mint_rate_limit_rejects_zero_window() public {
        vm.prank(admin);
        vm.expectRevert(BridgedIcUSD.InvalidConfig.selector);
        token.setMintRateLimit(1, 0);
        assertEq(token.mintWindowSeconds(), WINDOW_SECONDS);
    }

    function test_only_admin_can_grant_and_revoke_roles() public {
        vm.prank(guardian);
        vm.expectRevert(_unauthorized(guardian, ADMIN_ROLE));
        token.grantRole(MINTER, guardian);
        vm.prank(minter);
        vm.expectRevert(_unauthorized(minter, ADMIN_ROLE));
        token.grantRole(MINTER, alice);

        vm.prank(admin);
        token.grantRole(PAUSER, alice);
        assertTrue(token.hasRole(PAUSER, alice));
        vm.prank(admin);
        token.revokeRole(PAUSER, alice);
        assertFalse(token.hasRole(PAUSER, alice));
    }

    function test_admin_is_a_full_mint_authority_path() public {
        // Documented property: the admin can grant itself MINTER_ROLE, so it must only be held by the canister.
        vm.prank(admin);
        token.grantRole(MINTER, admin);
        vm.prank(admin);
        token.mint(alice, 1, bytes32(uint256(1)));
        assertEq(token.balanceOf(alice), 1);

        // And can revoke the original minter.
        vm.prank(admin);
        token.revokeRole(MINTER, minter);
        vm.prank(minter);
        vm.expectRevert(_unauthorized(minter, MINTER));
        token.mint(alice, 1, bytes32(uint256(2)));
    }

    function test_default_admin_role_cannot_be_granted_or_revoked_directly() public {
        vm.prank(admin);
        vm.expectRevert(IAccessControlDefaultAdminRules.AccessControlEnforcedDefaultAdminRules.selector);
        token.grantRole(ADMIN_ROLE, alice);
        vm.prank(admin);
        vm.expectRevert(IAccessControlDefaultAdminRules.AccessControlEnforcedDefaultAdminRules.selector);
        token.revokeRole(ADMIN_ROLE, admin);
    }

    function test_admin_delayed_transfer() public {
        address newAdmin = address(0xAD2);

        vm.prank(admin);
        token.beginDefaultAdminTransfer(newAdmin);
        (address pending, uint48 schedule) = token.pendingDefaultAdmin();
        assertEq(pending, newAdmin);
        assertEq(schedule, uint48(block.timestamp) + ADMIN_DELAY);

        // Immediately: delay not elapsed.
        vm.prank(newAdmin);
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControlDefaultAdminRules.AccessControlEnforcedDefaultAdminDelay.selector, schedule
            )
        );
        token.acceptDefaultAdminTransfer();

        // Exactly at the schedule: OpenZeppelin requires strictly after.
        vm.warp(schedule);
        vm.prank(newAdmin);
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControlDefaultAdminRules.AccessControlEnforcedDefaultAdminDelay.selector, schedule
            )
        );
        token.acceptDefaultAdminTransfer();

        // After the delay only the pending admin can accept.
        vm.warp(uint256(schedule) + 1);
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControlDefaultAdminRules.AccessControlInvalidDefaultAdmin.selector, alice)
        );
        token.acceptDefaultAdminTransfer();

        vm.prank(newAdmin);
        token.acceptDefaultAdminTransfer();

        assertEq(token.defaultAdmin(), newAdmin);
        assertTrue(token.hasRole(ADMIN_ROLE, newAdmin));
        assertFalse(token.hasRole(ADMIN_ROLE, admin));

        // Old admin can no longer use admin powers; new admin can.
        vm.prank(admin);
        vm.expectRevert(_unauthorized(admin, ADMIN_ROLE));
        token.setSupplyCap(1);
        vm.prank(newAdmin);
        token.setSupplyCap(1);
        assertEq(token.supplyCap(), 1);

        // A transfer moves only DEFAULT_ADMIN_ROLE. The old admin keeps its pause-only
        // role until the new admin revokes it; the new admin has none until granted.
        assertTrue(token.hasRole(PAUSER, admin));
        assertFalse(token.hasRole(PAUSER, newAdmin));
        vm.prank(newAdmin);
        token.revokeRole(PAUSER, admin);
        assertFalse(token.hasRole(PAUSER, admin));
    }

    function test_admin_transfer_can_be_cancelled() public {
        address newAdmin = address(0xAD2);
        vm.prank(admin);
        token.beginDefaultAdminTransfer(newAdmin);
        vm.prank(admin);
        token.cancelDefaultAdminTransfer();
        vm.warp(block.timestamp + ADMIN_DELAY + 1);
        vm.prank(newAdmin);
        vm.expectRevert();
        token.acceptDefaultAdminTransfer();
        assertEq(token.defaultAdmin(), admin);
    }

    function test_only_admin_can_begin_admin_transfer() public {
        vm.prank(minter);
        vm.expectRevert(_unauthorized(minter, ADMIN_ROLE));
        token.beginDefaultAdminTransfer(minter);
    }

    // ------------------------------------------------------------------
    // Standard ERC-20 behaviour is retained
    // ------------------------------------------------------------------

    function test_standard_erc20_flow() public {
        _mint(alice, 1_000, bytes32(uint256(1)));
        vm.prank(alice);
        assertTrue(token.transfer(bob, 400));
        assertEq(token.balanceOf(alice), 600);
        assertEq(token.balanceOf(bob), 400);

        vm.prank(bob);
        assertTrue(token.approve(carol, 150));
        assertEq(token.allowance(bob, carol), 150);
        vm.prank(carol);
        assertTrue(token.transferFrom(bob, carol, 150));
        assertEq(token.balanceOf(carol), 150);
        assertEq(token.totalSupply(), 1_000);
    }

    // ------------------------------------------------------------------
    // Raw log layout (what the Rust decoder reads)
    // ------------------------------------------------------------------

    function test_raw_log_layout_bridge_mint() public {
        bytes32 id = keccak256("raw-mint");
        uint256 amount = 2_500_000_000;
        vm.recordLogs();
        _mint(alice, amount, id);
        Vm.Log[] memory logs = vm.getRecordedLogs();

        assertEq(logs.length, 2, "Transfer then BridgeMint");

        // Log 0: ERC-20 Transfer(0x0, alice, amount).
        assertEq(logs[0].emitter, address(token));
        assertEq(logs[0].topics.length, 3);
        assertEq(logs[0].topics[0], 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef);
        assertEq(logs[0].topics[1], bytes32(0));
        assertEq(logs[0].topics[2], bytes32(uint256(uint160(alice))));
        assertEq(logs[0].data, abi.encode(amount));

        // Log 1: BridgeMint, emitted AFTER the Transfer.
        assertEq(logs[1].emitter, address(token));
        assertEq(logs[1].topics.length, 3, "BridgeMint has exactly 3 topics");
        assertEq(logs[1].topics[0], 0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc);
        assertEq(logs[1].topics[1], id);
        assertEq(logs[1].topics[2], bytes32(uint256(uint160(alice))));
        assertEq(logs[1].data, abi.encode(amount));
        assertEq(logs[1].data, hex"000000000000000000000000000000000000000000000000000000009502f900");
    }

    function test_raw_log_layout_bridge_out() public {
        _mint(alice, 10_000_000_000, bytes32(uint256(1)));
        uint256 amount = 2_500_000_000;

        vm.recordLogs();
        bytes32 id = _bridgeOut(alice, amount, NONCE);
        Vm.Log[] memory logs = vm.getRecordedLogs();

        assertEq(logs.length, 2, "Transfer(burn) then BridgeOut");
        assertEq(logs[0].topics[0], 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef);
        assertEq(logs[0].topics[2], bytes32(0));

        Vm.Log memory l = logs[1];
        assertEq(l.emitter, address(token));
        assertEq(l.topics.length, 3, "BridgeOut has exactly 3 topics");
        assertEq(l.topics[0], 0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b);
        assertEq(l.topics[1], id);
        assertEq(l.topics[2], bytes32(uint256(uint160(alice))));

        // data = abi.encode(amount, sequence, icpPrincipal, icpSubaccount):
        //   word0 amount | word1 sequence | word2 offset(0x80) | word3 subaccount
        //   word4 principal length | word5 principal bytes (right-padded)
        bytes memory expected = abi.encodePacked(
            uint256(amount),
            uint256(1),
            uint256(0x80),
            SUBACCOUNT,
            uint256(10),
            PRINCIPAL,
            bytes22(0) // right padding to a full word
        );
        assertEq(l.data.length, 6 * 32);
        assertEq(l.data, expected);
        assertEq(l.data, abi.encode(amount, uint64(1), PRINCIPAL, SUBACCOUNT));
        // Pinned literal produced by `cast abi-encode "f(uint256,uint64,bytes,bytes32)"`.
        assertEq(
            l.data,
            hex"000000000000000000000000000000000000000000000000000000009502f900"
            hex"0000000000000000000000000000000000000000000000000000000000000001"
            hex"0000000000000000000000000000000000000000000000000000000000000080"
            hex"0000000000000000000000000000000000000000000000000000000000000abc"
            hex"000000000000000000000000000000000000000000000000000000000000000a"
            hex"0000000000000002010100000000000000000000000000000000000000000000"
        );
        // Offset word and padding are explicit.
        assertEq(uint256(bytes32(_slice(l.data, 64, 32))), 0x80);
    }

    function test_raw_log_layout_bridge_out_29_byte_principal() public {
        _mint(alice, 10_000_000_000, bytes32(uint256(1)));
        bytes memory p29 = new bytes(29);
        for (uint256 i = 0; i < 29; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            p29[i] = bytes1(uint8(0xA0 + i));
        }
        vm.recordLogs();
        vm.prank(alice);
        token.bridgeOut(MIN_BRIDGE_OUT, p29, bytes32(0), NONCE);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        Vm.Log memory l = logs[1];
        assertEq(l.data.length, 6 * 32);
        assertEq(l.data, abi.encode(MIN_BRIDGE_OUT, uint64(1), p29, bytes32(0)));
        // length word is 29 and the last 3 bytes of the data word are zero padding.
        assertEq(uint256(bytes32(_slice(l.data, 128, 32))), 29);
        bytes memory dataWord = _slice(l.data, 160, 32);
        assertEq(uint8(dataWord[0]), 0xA0);
        assertEq(uint8(dataWord[28]), 0xA0 + 28);
        assertEq(uint8(dataWord[29]), 0);
        assertEq(uint8(dataWord[30]), 0);
        assertEq(uint8(dataWord[31]), 0);
    }

    function _slice(bytes memory b, uint256 start, uint256 len) internal pure returns (bytes memory out) {
        out = new bytes(len);
        for (uint256 i = 0; i < len; i++) {
            out[i] = b[start + i];
        }
    }

    // ------------------------------------------------------------------
    // ABI pin: selectors, topic0 and constants a Rust decoder and TS encoder depend on
    // ------------------------------------------------------------------

    function test_abi_pin_event_topics() public pure {
        assertEq(
            keccak256("BridgeOut(bytes32,address,uint256,uint64,bytes,bytes32)"),
            0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b
        );
        assertEq(BridgeOut.selector, 0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b);
        assertEq(
            keccak256("BridgeMint(bytes32,address,uint256)"),
            0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc
        );
        assertEq(BridgeMint.selector, 0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc);
        assertEq(keccak256("MintPausedSet(bool)"), 0x0fb79081612032f5ce63340f421121d9f8b8ea04158f08b2dfafacb3393caf98);
        assertEq(
            keccak256("BridgeOutPausedSet(bool)"), 0x13392ee7d9f1b5e8c2a6ef3f54ce0a66b6db278e86b3160a4bbe4006e0cb9589
        );
        assertEq(keccak256("SupplyCapSet(uint256)"), 0xbc585eed6f54aa16ec292be93276937803d5a047ba4eded0c87779270bbfdfe6);
        assertEq(
            keccak256("MintRateLimitSet(uint256,uint64)"),
            0xa2ec63fd54fcc61cffa4f457af25daa2854e5c39236e6ba3d51b540a52e95039
        );
        assertEq(
            keccak256("MinBridgeOutSet(uint256)"), 0x9c20235b1fc2597395d832344f76d44d6e82f97345c9cd4a084bfa78df69348c
        );
    }

    function test_abi_pin_selectors() public view {
        assertEq(bytes4(keccak256("mint(address,uint256,bytes32)")), bytes4(0x1e458bee));
        assertEq(BridgedIcUSD.mint.selector, bytes4(0x1e458bee));

        assertEq(bytes4(keccak256("bridgeOut(uint256,bytes,bytes32,bytes32)")), bytes4(0x803451ee));
        assertEq(BridgedIcUSD.bridgeOut.selector, bytes4(0x803451ee));

        assertEq(bytes4(keccak256("computeTransferId(address,bytes32)")), bytes4(0xaee78e6a));
        assertEq(BridgedIcUSD.computeTransferId.selector, bytes4(0xaee78e6a));

        assertEq(bytes4(keccak256("pauseMint()")), bytes4(0xcd85cdb5));
        assertEq(BridgedIcUSD.pauseMint.selector, bytes4(0xcd85cdb5));

        assertEq(bytes4(keccak256("unpauseMint()")), bytes4(0x1a8bd2da));
        assertEq(BridgedIcUSD.unpauseMint.selector, bytes4(0x1a8bd2da));

        assertEq(bytes4(keccak256("pauseBridgeOut()")), bytes4(0x2f1d448f));
        assertEq(BridgedIcUSD.pauseBridgeOut.selector, bytes4(0x2f1d448f));

        assertEq(bytes4(keccak256("unpauseBridgeOut()")), bytes4(0x7b455e06));
        assertEq(BridgedIcUSD.unpauseBridgeOut.selector, bytes4(0x7b455e06));

        assertEq(bytes4(keccak256("setSupplyCap(uint256)")), bytes4(0xb6a3f59a));
        assertEq(BridgedIcUSD.setSupplyCap.selector, bytes4(0xb6a3f59a));

        assertEq(bytes4(keccak256("setMintRateLimit(uint256,uint64)")), bytes4(0x1a5b7916));
        assertEq(BridgedIcUSD.setMintRateLimit.selector, bytes4(0x1a5b7916));

        assertEq(bytes4(keccak256("setMinBridgeOut(uint256)")), bytes4(0xccf84ef8));
        assertEq(BridgedIcUSD.setMinBridgeOut.selector, bytes4(0xccf84ef8));

        assertEq(bytes4(keccak256("mintConsumed(bytes32)")), bytes4(0x26c5b40e));
        assertEq(token.mintConsumed.selector, bytes4(0x26c5b40e));

        assertEq(bytes4(keccak256("bridgeOutUsed(bytes32)")), bytes4(0x3e942d93));
        assertEq(token.bridgeOutUsed.selector, bytes4(0x3e942d93));

        assertEq(bytes4(keccak256("bridgeOutCount()")), bytes4(0x53ebd818));
        assertEq(token.bridgeOutCount.selector, bytes4(0x53ebd818));

        assertEq(bytes4(keccak256("totalSupply()")), bytes4(0x18160ddd));
        assertEq(token.totalSupply.selector, bytes4(0x18160ddd));

        assertEq(bytes4(keccak256("supplyCap()")), bytes4(0x8f770ad0));
        assertEq(token.supplyCap.selector, bytes4(0x8f770ad0));

        assertEq(bytes4(keccak256("VERSION()")), bytes4(0xffa1ad74));
        assertEq(token.VERSION.selector, bytes4(0xffa1ad74));

        assertEq(bytes4(keccak256("hasRole(bytes32,address)")), bytes4(0x91d14854));
        assertEq(token.hasRole.selector, bytes4(0x91d14854));

        assertEq(bytes4(keccak256("defaultAdmin()")), bytes4(0x84ef8ffc));
        assertEq(token.defaultAdmin.selector, bytes4(0x84ef8ffc));

        assertEq(bytes4(keccak256("decimals()")), bytes4(0x313ce567));
        assertEq(token.decimals.selector, bytes4(0x313ce567));
    }

    function test_abi_pin_constants() public view {
        assertEq(token.BRIDGE_OUT_DOMAIN(), 0xbf6555f205691a2cd4c2403b214c0d516fdb66c929882e857755b63e90f1d4f1);
        assertEq(
            keccak256("rumi.icusd.base.bridge-out.v1"),
            0xbf6555f205691a2cd4c2403b214c0d516fdb66c929882e857755b63e90f1d4f1
        );
        assertEq(token.MINTER_ROLE(), 0x9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6);
        assertEq(token.PAUSER_ROLE(), 0x65d7a28e3265b37a6474929f336521b332c1681b933f6cb9f3376673440d862a);
        assertEq(token.DEFAULT_ADMIN_ROLE(), bytes32(0));
        assertEq(token.VERSION(), "rumi-base-icusd-bridge-v1");
        assertEq(token.ICP_PRINCIPAL_MAX_LEN(), 29);
    }

    // ------------------------------------------------------------------
    // Fuzz
    // ------------------------------------------------------------------

    function testFuzz_mint_amount_boundaries(uint256 amount) public {
        BridgedIcUSD t = new BridgedIcUSD(
            admin, ADMIN_DELAY, minter, guardian, type(uint256).max, type(uint256).max, 1 days, MIN_BRIDGE_OUT
        );
        vm.prank(minter);
        if (amount == 0) {
            vm.expectRevert(BridgedIcUSD.ZeroAmount.selector);
        } else if (amount > type(uint64).max) {
            vm.expectRevert(abi.encodeWithSelector(BridgedIcUSD.AmountTooLarge.selector, amount));
        }
        t.mint(alice, amount, bytes32(uint256(1)));
        if (amount != 0 && amount <= type(uint64).max) {
            assertEq(t.balanceOf(alice), amount);
            assertEq(t.totalSupply(), amount);
        } else {
            assertEq(t.totalSupply(), 0);
        }
    }

    function testFuzz_supply_cap_boundary(uint64 cap, uint64 amount) public {
        vm.assume(amount != 0);
        BridgedIcUSD t =
            new BridgedIcUSD(admin, ADMIN_DELAY, minter, guardian, cap, type(uint256).max, 1 days, MIN_BRIDGE_OUT);
        vm.prank(minter);
        if (amount > cap) {
            vm.expectRevert(
                abi.encodeWithSelector(BridgedIcUSD.SupplyCapExceeded.selector, uint256(amount), uint256(cap))
            );
        }
        t.mint(alice, amount, bytes32(uint256(1)));
        assertEq(t.totalSupply(), amount > cap ? 0 : amount);
    }

    function testFuzz_rate_limit_boundary(uint64 limit, uint64 amount) public {
        vm.assume(amount != 0);
        BridgedIcUSD t =
            new BridgedIcUSD(admin, ADMIN_DELAY, minter, guardian, type(uint256).max, limit, 1 days, MIN_BRIDGE_OUT);
        vm.prank(minter);
        if (amount > limit) {
            vm.expectRevert(
                abi.encodeWithSelector(BridgedIcUSD.MintRateLimitExceeded.selector, uint256(amount), uint256(limit))
            );
        }
        t.mint(alice, amount, bytes32(uint256(1)));
        assertEq(t.mintedInWindow(), amount > limit ? 0 : amount);
    }

    function testFuzz_bridge_out_amount_boundaries(uint256 amount, uint64 balance, uint64 minimum) public {
        BridgedIcUSD t = new BridgedIcUSD(
            admin, ADMIN_DELAY, minter, guardian, type(uint256).max, type(uint256).max, 1 days, minimum
        );
        if (balance != 0) {
            vm.prank(minter);
            t.mint(alice, balance, bytes32(uint256(1)));
        }
        bytes memory expectedRevert;
        if (amount == 0) {
            expectedRevert = abi.encodeWithSelector(BridgedIcUSD.ZeroAmount.selector);
        } else if (amount < minimum) {
            expectedRevert = abi.encodeWithSelector(BridgedIcUSD.AmountBelowMinimum.selector, amount, uint256(minimum));
        } else if (amount > type(uint64).max) {
            expectedRevert = abi.encodeWithSelector(BridgedIcUSD.AmountTooLarge.selector, amount);
        } else if (amount > balance) {
            expectedRevert =
                abi.encodeWithSelector(IERC20Errors.ERC20InsufficientBalance.selector, alice, uint256(balance), amount);
        }
        vm.prank(alice);
        if (expectedRevert.length != 0) {
            vm.expectRevert(expectedRevert);
        }
        t.bridgeOut(amount, PRINCIPAL, SUBACCOUNT, NONCE);
        if (expectedRevert.length == 0) {
            assertEq(t.balanceOf(alice), uint256(balance) - amount);
            assertEq(t.bridgeOutCount(), 1);
        } else {
            assertEq(t.balanceOf(alice), balance);
            assertEq(t.bridgeOutCount(), 0);
        }
    }

    function testFuzz_principal_length(uint8 len) public {
        _mint(alice, 100_000_000, bytes32(uint256(1)));
        bytes memory p = new bytes(len);
        if (len > 0) p[0] = 0x07;
        vm.prank(alice);
        if (len == 0 || len > 29) {
            vm.expectRevert(BridgedIcUSD.InvalidIcpPrincipal.selector);
        }
        token.bridgeOut(MIN_BRIDGE_OUT, p, SUBACCOUNT, NONCE);
    }

    /// @dev Random interleavings of mints, bridge-outs and time jumps. Successes are
    /// tracked by try/catch; supply and the counter must always match the model.
    function testFuzz_random_sequences_keep_supply_and_counter_consistent(uint256 seed) public {
        address[3] memory actors = [alice, bob, carol];
        uint256 minted;
        uint256 burned;
        uint64 successfulOuts;
        uint256 idCounter;
        uint256 successfulMints;

        for (uint256 i = 0; i < 48; i++) {
            uint256 r = uint256(keccak256(abi.encode(seed, i)));
            uint256 op = r % 4;
            address actor = actors[(r >> 8) % 3];
            uint256 amount = (r >> 16) % 2_000_000_000_000; // up to 20k icUSD, may be zero
            if ((r >> 100) % 8 == 0) amount = (r >> 16) % MIN_BRIDGE_OUT; // sub-minimum / zero
            if ((r >> 104) % 16 == 0) amount = uint256(type(uint64).max) + ((r >> 16) % 3); // boundary / overflow

            if (op <= 1) {
                // Occasionally replay an older mint id.
                bytes32 id = bytes32(((r >> 110) % 6 == 0 && idCounter > 0) ? idCounter - 1 : idCounter++);
                vm.prank(minter);
                try token.mint(actor, amount, id) {
                    minted += amount;
                    successfulMints++;
                } catch {}
            } else if (op == 2) {
                bytes32 nonce = bytes32(((r >> 120) % 6 == 0 && idCounter > 0) ? idCounter - 1 : idCounter++);
                vm.prank(actor);
                try token.bridgeOut(amount, PRINCIPAL, SUBACCOUNT, nonce) {
                    burned += amount;
                    successfulOuts++;
                } catch {}
            } else {
                vm.warp(block.timestamp + ((r >> 130) % (2 * uint256(WINDOW_SECONDS))));
            }

            assertEq(token.totalSupply(), minted - burned, "supply != minted - burned");
            assertEq(token.bridgeOutCount(), successfulOuts, "bridgeOutCount != successful bridgeOuts");
            assertLe(token.totalSupply(), token.supplyCap(), "supply above cap");
            assertEq(
                token.balanceOf(alice) + token.balanceOf(bob) + token.balanceOf(carol),
                token.totalSupply(),
                "balances do not sum to supply"
            );
        }
    }
}

/// @dev Drives the token through random valid and invalid calls and keeps ghost totals.
contract BridgedIcUSDHandler is Test {
    BridgedIcUSD public token;
    address public admin;
    address public minter;
    address[] public actors;

    uint256 public ghostMinted;
    uint256 public ghostBurned;
    uint64 public ghostBridgeOuts;
    uint256 public ghostMintSuccesses;
    uint256 public ghostMintReverts;
    uint256 public nextId;

    bytes constant PRINCIPAL = hex"00000000000000020101";

    constructor(BridgedIcUSD token_, address admin_, address minter_, address[] memory actors_) {
        token = token_;
        admin = admin_;
        minter = minter_;
        actors = actors_;
    }

    function _actor(uint256 seed) internal view returns (address) {
        return actors[seed % actors.length];
    }

    function mint(uint256 actorSeed, uint256 amount, uint256 idSeed) external {
        amount = bound(amount, 0, 12_000e8);
        address to = _actor(actorSeed);
        // One mint in eight replays the previous id (must be rejected if it was consumed).
        bytes32 id = bytes32(idSeed % 8 == 0 && nextId > 0 ? nextId - 1 : nextId++);
        vm.prank(minter);
        try token.mint(to, amount, id) {
            ghostMinted += amount;
            ghostMintSuccesses++;
        } catch {
            ghostMintReverts++;
        }
    }

    function bridgeOut(uint256 actorSeed, uint256 amount) external {
        address from = _actor(actorSeed);
        amount = bound(amount, 0, token.balanceOf(from) + 1);
        bytes32 nonce = bytes32(nextId++);
        vm.prank(from);
        try token.bridgeOut(amount, PRINCIPAL, bytes32(0), nonce) {
            ghostBurned += amount;
            ghostBridgeOuts++;
        } catch {}
    }

    function transferBetween(uint256 fromSeed, uint256 toSeed, uint256 amount) external {
        address from = _actor(fromSeed);
        address to = _actor(toSeed);
        amount = bound(amount, 0, token.balanceOf(from));
        vm.prank(from);
        assertTrue(token.transfer(to, amount));
    }

    function warp(uint256 secs) external {
        vm.warp(block.timestamp + bound(secs, 0, 3 days));
    }

    /// @dev Only ever raises (or holds) the cap relative to the live supply, so
    /// `totalSupply <= supplyCap` stays an invariant of the mint path.
    function adjustSupplyCap(uint256 extra) external {
        uint256 newCap = token.totalSupply() + bound(extra, 0, 200_000e8);
        vm.prank(admin);
        token.setSupplyCap(newCap);
    }

    function adjustRateLimit(uint256 limit, uint256 windowSeconds) external {
        vm.prank(admin);
        // forge-lint: disable-next-line(unsafe-typecast)
        token.setMintRateLimit(bound(limit, 20_000e8, 100_000e8), uint64(bound(windowSeconds, 1, 3 days)));
    }

    function togglePauses(uint8 which) external {
        // Mostly unpause so the run keeps making progress; pausing is the rarer case.
        which = which % 8;
        if (which == 0) {
            vm.prank(admin);
            token.pauseMint();
        } else if (which <= 3) {
            vm.prank(admin);
            token.unpauseMint();
        } else if (which == 4) {
            vm.prank(admin);
            token.pauseBridgeOut();
        } else {
            vm.prank(admin);
            token.unpauseBridgeOut();
        }
    }
}

/// forge-config: default.invariant.runs = 128
/// forge-config: default.invariant.depth = 250
contract BridgedIcUSDInvariantTest is StdInvariant, Test {
    BridgedIcUSD token;
    BridgedIcUSDHandler handler;
    address[] actors;

    function setUp() public {
        address admin = address(0xA11CE);
        address minter = address(0xB0B);
        token = new BridgedIcUSD(admin, 1 days, minter, address(0x6A2D), 150_000e8, 60_000e8, 1 days, 1e6);
        actors.push(address(0xCAFE));
        actors.push(address(0xBEEF));
        actors.push(address(0xCA201));
        actors.push(address(0xD00D));
        handler = new BridgedIcUSDHandler(token, admin, minter, actors);
        targetContract(address(handler));
        // Weighted: mints and bridge-outs are registered more than once so the
        // run reaches deep, successful state instead of mostly bouncing off pauses.
        bytes4[] memory selectors = new bytes4[](12);
        selectors[0] = BridgedIcUSDHandler.mint.selector;
        selectors[1] = BridgedIcUSDHandler.mint.selector;
        selectors[2] = BridgedIcUSDHandler.mint.selector;
        selectors[3] = BridgedIcUSDHandler.mint.selector;
        selectors[4] = BridgedIcUSDHandler.bridgeOut.selector;
        selectors[5] = BridgedIcUSDHandler.bridgeOut.selector;
        selectors[6] = BridgedIcUSDHandler.transferBetween.selector;
        selectors[7] = BridgedIcUSDHandler.warp.selector;
        selectors[8] = BridgedIcUSDHandler.adjustSupplyCap.selector;
        selectors[9] = BridgedIcUSDHandler.adjustRateLimit.selector;
        selectors[10] = BridgedIcUSDHandler.togglePauses.selector;
        selectors[11] = BridgedIcUSDHandler.warp.selector;
        targetSelector(FuzzSelector({addr: address(handler), selectors: selectors}));
    }

    /// totalSupply never exceeds the supply cap: mints are the only way supply
    /// grows, and the handler never sets the cap below the live supply.
    function invariant_supply_never_exceeds_cap() public view {
        assertLe(token.totalSupply(), token.supplyCap());
    }

    function invariant_supply_equals_ghost_minted_minus_burned() public view {
        assertEq(token.totalSupply(), handler.ghostMinted() - handler.ghostBurned());
    }

    function invariant_bridge_out_count_matches_successes() public view {
        assertEq(token.bridgeOutCount(), handler.ghostBridgeOuts());
    }

    function invariant_balances_sum_to_supply() public view {
        uint256 sum;
        for (uint256 i = 0; i < actors.length; i++) {
            sum += token.balanceOf(actors[i]);
        }
        assertEq(sum, token.totalSupply());
    }
}
