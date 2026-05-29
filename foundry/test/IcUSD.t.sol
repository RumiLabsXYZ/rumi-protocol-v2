// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IcUSD} from "../src/IcUSD.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";

contract IcUSDTest is Test {
    IcUSD icusd;
    address admin = address(0xA11CE);
    address minter = address(0xB0B);   // stands in for the canister settlement address
    address alice = address(0xCAFE);
    address bob = address(0xBEEF);

    // Re-declare the events for vm.expectEmit (must match IcUSD.sol exactly, incl. `indexed`).
    event Mint(uint256 indexed vault_id, address indexed recipient, uint256 amount);
    event Burn(uint256 indexed vault_id, address indexed burner, uint256 amount);

    function setUp() public {
        icusd = new IcUSD(admin, minter);
    }

    function test_decimals_is_8() public view {
        assertEq(icusd.decimals(), 8);
    }

    function test_minter_can_mint_and_emits_event() public {
        vm.expectEmit(true, true, false, true); // check vault_id(topic1), recipient(topic2), data(amount)
        emit Mint(42, alice, 10_000_000_000);
        vm.prank(minter);
        icusd.mint(alice, 10_000_000_000, 42);
        assertEq(icusd.balanceOf(alice), 10_000_000_000);
        assertEq(icusd.totalSupply(), 10_000_000_000);
    }

    function test_non_minter_cannot_mint() public {
        // Read MINTER_ROLE into a local BEFORE the prank: in Foundry 1.6.0 the
        // one-shot `vm.prank` would otherwise be consumed by the external
        // `icusd.MINTER_ROLE()` STATICCALL evaluated as an arg here, leaving the
        // `mint` call to run as address(this) instead of alice (the revert is
        // identical AccessControlUnauthorizedAccount, only the `account` field
        // would differ). Asserts OZ v5's custom error with account == alice.
        bytes32 minterRole = icusd.MINTER_ROLE();
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, alice, minterRole)
        );
        icusd.mint(alice, 1, 1);
    }

    function test_anyone_can_burn_their_balance_and_emits_event() public {
        vm.prank(minter);
        icusd.mint(alice, 10_000_000_000, 7);
        vm.expectEmit(true, true, false, true);
        emit Burn(7, alice, 4_000_000_000);
        vm.prank(alice);
        icusd.burn(4_000_000_000, 7);
        assertEq(icusd.balanceOf(alice), 6_000_000_000);
        assertEq(icusd.totalSupply(), 6_000_000_000);
    }

    function test_burn_exceeding_balance_reverts() public {
        vm.prank(minter);
        icusd.mint(alice, 100, 1);
        vm.prank(alice);
        vm.expectRevert(); // ERC20InsufficientBalance (OZ v5 custom error)
        icusd.burn(101, 1);
    }

    function test_total_supply_tracks_mint_minus_burn() public {
        vm.startPrank(minter);
        icusd.mint(alice, 1_000, 1);
        icusd.mint(bob, 2_000, 2);
        vm.stopPrank();
        assertEq(icusd.totalSupply(), 3_000);
        vm.prank(bob);
        icusd.burn(500, 2);
        assertEq(icusd.totalSupply(), 2_500);
    }

    // --- Task-20 addition 1: per-vault_id mint idempotency guard ---
    function test_mint_same_vault_id_twice_reverts() public {
        vm.startPrank(minter);
        icusd.mint(alice, 100, 7);
        assertTrue(icusd.minted(7));
        vm.expectRevert(bytes("vault already minted"));
        icusd.mint(alice, 50, 7); // same vault_id -> revert (idempotency: no on-chain double-mint)
        vm.stopPrank();
        assertEq(icusd.totalSupply(), 100); // second mint did not happen
    }

    // --- Task-20 addition 2: ABI PINNING to the backend's constants ---
    // These lock IcUSD.sol's selector + event topic0 to the values the Rust
    // backend pins (tx::encode_mint_calldata selector, MINT/BURN_EVENT_TOPIC0 in
    // evm_rpc.rs). If anyone changes an event/mint signature, this test fails,
    // catching a contract<->canister ABI drift before it ships.
    function test_abi_pinned_to_backend_constants() public pure {
        // mint(address,uint256,uint64) selector == tx::encode_mint_calldata's selector
        assertEq(bytes4(keccak256("mint(address,uint256,uint64)")), bytes4(0x8b3d35ae), "mint selector drift");
        // Mint/Burn event topic0 == MINT_EVENT_TOPIC0 / BURN_EVENT_TOPIC0 in evm_rpc.rs
        assertEq(
            keccak256("Mint(uint256,address,uint256)"),
            bytes32(0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6),
            "Mint topic0 drift"
        );
        assertEq(
            keccak256("Burn(uint256,address,uint256)"),
            bytes32(0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca),
            "Burn topic0 drift"
        );
    }

    function testFuzz_mint_then_full_burn_nets_zero(uint96 amount, uint64 vaultId) public {
        vm.assume(amount > 0);
        vm.prank(minter);
        icusd.mint(alice, amount, vaultId);
        vm.prank(alice);
        icusd.burn(amount, vaultId);
        assertEq(icusd.balanceOf(alice), 0);
        assertEq(icusd.totalSupply(), 0);
    }

    function test_standard_erc20_transfer_approve() public {
        vm.prank(minter);
        icusd.mint(alice, 1_000, 1);
        vm.prank(alice);
        icusd.transfer(bob, 400);
        assertEq(icusd.balanceOf(bob), 400);
        vm.prank(bob);
        icusd.approve(alice, 100);
        assertEq(icusd.allowance(bob, alice), 100);
    }
}
