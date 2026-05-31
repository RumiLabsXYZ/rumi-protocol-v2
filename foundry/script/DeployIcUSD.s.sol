// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {IcUSD} from "../src/IcUSD.sol";

/// Deploy IcUSD to Monad testnet. The canister settlement address (from
/// get_chain_settlement_address on the backend) is BOTH admin + minter for
/// Phase 1b — the canister owns the token from block zero and is the sole minter.
/// Env vars (set before running):
///   MONAD_TESTNET_RPC        - Monad testnet RPC URL
///   DEPLOYER_PK              - a funded testnet deployer private key (pays gas)
///   CANISTER_SETTLEMENT_ADDR - the backend's settlement address (0x..)
contract DeployIcUSD is Script {
    function run() external {
        address settlement = vm.envAddress("CANISTER_SETTLEMENT_ADDR");
        uint256 deployerPk = vm.envUint("DEPLOYER_PK");
        vm.startBroadcast(deployerPk);
        IcUSD icusd = new IcUSD(settlement, settlement); // admin == minter == canister
        vm.stopBroadcast();
        console2.log("IcUSD deployed at:", address(icusd));
        console2.log("admin + minter:", settlement);
    }
}
