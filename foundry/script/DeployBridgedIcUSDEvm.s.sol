// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

// Sibling of DeployBridgedIcUSD.s.sol for the other EVM chains the bridge canister can serve
// (Ethereum, Arbitrum, Monad). It deploys the IDENTICAL BridgedIcUSD bytecode (decision D5):
// same contract, same constructor, same VERSION string. Only the chain guard differs, and
// Base keeps its own script, which this one refuses to stand in for.
//
// SAFETY: this script must NEVER be run with --broadcast against a public network
// without separate, explicit authorization from the protocol owner. Dry runs (no
// --broadcast) and local anvil runs are fine. Each mainnet needs its own flag (below), so
// authorizing one network never authorizes another.
//
// The script reads NO private key from the environment. The operator supplies the
// signer on the command line at run time (--account <keystore> or --private-key),
// and that deployer must be a throwaway account that is NOT the admin, the minter
// or the guardian: the token grants the deployer no role, and the script refuses
// to run if the deployer is one of the configured role holders.
//
// Required env vars:
//   EXPECTED_CHAIN_ID               - must equal block.chainid (guards against a wrong --rpc-url)
//   BRIDGE_ADMIN_ADDR               - DEFAULT_ADMIN_ROLE holder (bridge canister tECDSA admin address)
//   BRIDGE_MINTER_ADDR              - MINTER_ROLE holder (bridge canister tECDSA minter address)
//   BRIDGE_ADMIN_DELAY_SECONDS      - delay for the two-step admin transfer
//   BRIDGE_SUPPLY_CAP_E8S           - max totalSupply after a mint (e8s)
//   BRIDGE_MINT_WINDOW_LIMIT_E8S    - max amount minted per window (e8s)
//   BRIDGE_MINT_WINDOW_SECONDS      - mint rate-limit window length (> 0)
//   BRIDGE_MIN_BRIDGE_OUT_E8S       - minimum bridgeOut amount (e8s, > 0)
// Optional env vars:
//   BRIDGE_GUARDIAN_ADDR            - pause-only account (default: none)
//   ALLOW_ETHEREUM_MAINNET          - must be exactly true to allow chain id 1 (default: false)
//   ALLOW_ARBITRUM_MAINNET          - must be exactly true to allow chain id 42161 (default: false)
//   ALLOW_MONAD_MAINNET             - must be exactly true to allow chain id 143 (default: false)
//
// Allowed chains: 11155111 (Ethereum Sepolia), 421614 (Arbitrum Sepolia), 10143 (Monad testnet),
// 31337 (local anvil), and 1, 42161, 143 only with their own flag. Every other chain id is
// rejected, including Base (8453, 84532: use DeployBridgedIcUSD), Robinhood Chain and HyperEVM.
//
// Monad charges gas_limit x price rather than gas used: read the estimate before broadcasting.

import {Script, console2} from "forge-std/Script.sol";
import {BridgedIcUSD} from "../src/BridgedIcUSD.sol";

contract DeployBridgedIcUSDEvm is Script {
    uint256 internal constant ETHEREUM_MAINNET_CHAIN_ID = 1;
    uint256 internal constant ETHEREUM_SEPOLIA_CHAIN_ID = 11155111;
    uint256 internal constant ARBITRUM_ONE_CHAIN_ID = 42161;
    uint256 internal constant ARBITRUM_SEPOLIA_CHAIN_ID = 421614;
    uint256 internal constant MONAD_MAINNET_CHAIN_ID = 143;
    uint256 internal constant MONAD_TESTNET_CHAIN_ID = 10143;
    uint256 internal constant LOCAL_ANVIL_CHAIN_ID = 31337;
    uint256 internal constant BASE_MAINNET_CHAIN_ID = 8453;
    uint256 internal constant BASE_SEPOLIA_CHAIN_ID = 84532;

    /// @dev Identical bytecode on every chain (D5), so the contract keeps its Base-named version.
    string internal constant EXPECTED_VERSION = "rumi-base-icusd-bridge-v1";

    struct Config {
        uint256 expectedChainId;
        bool allowEthereumMainnet;
        bool allowArbitrumMainnet;
        bool allowMonadMainnet;
        address admin;
        address minter;
        address guardian; // address(0) means no guardian
        uint48 adminDelaySeconds;
        uint256 supplyCapE8s;
        uint256 mintWindowLimitE8s;
        uint64 mintWindowSeconds;
        uint256 minBridgeOutE8s;
    }

    function run() external returns (BridgedIcUSD token) {
        token = deploy(loadConfig());
    }

    // Environment access goes through these hooks so the tests can supply values without
    // touching the process-wide environment (which other test contracts share).
    function _envUint(string memory name) internal view virtual returns (uint256) {
        return vm.envUint(name);
    }

    function _envAddress(string memory name) internal view virtual returns (address) {
        return vm.envAddress(name);
    }

    function _envAddressOr(string memory name, address fallbackValue) internal view virtual returns (address) {
        return vm.envOr(name, fallbackValue);
    }

    function _envBoolOr(string memory name, bool fallbackValue) internal view virtual returns (bool) {
        return vm.envOr(name, fallbackValue);
    }

    /// @notice Read and range-check the environment into a `Config`.
    function loadConfig() public view returns (Config memory cfg) {
        cfg.expectedChainId = _envUint("EXPECTED_CHAIN_ID");
        cfg.allowEthereumMainnet = _envBoolOr("ALLOW_ETHEREUM_MAINNET", false);
        cfg.allowArbitrumMainnet = _envBoolOr("ALLOW_ARBITRUM_MAINNET", false);
        cfg.allowMonadMainnet = _envBoolOr("ALLOW_MONAD_MAINNET", false);
        cfg.admin = _envAddress("BRIDGE_ADMIN_ADDR");
        cfg.minter = _envAddress("BRIDGE_MINTER_ADDR");
        cfg.guardian = _envAddressOr("BRIDGE_GUARDIAN_ADDR", address(0));

        uint256 delay = _envUint("BRIDGE_ADMIN_DELAY_SECONDS");
        require(delay <= type(uint48).max, "BRIDGE_ADMIN_DELAY_SECONDS exceeds uint48");
        // forge-lint: disable-next-line(unsafe-typecast)
        cfg.adminDelaySeconds = uint48(delay); // range-checked above

        cfg.supplyCapE8s = _envUint("BRIDGE_SUPPLY_CAP_E8S");
        cfg.mintWindowLimitE8s = _envUint("BRIDGE_MINT_WINDOW_LIMIT_E8S");

        uint256 windowSeconds = _envUint("BRIDGE_MINT_WINDOW_SECONDS");
        require(windowSeconds <= type(uint64).max, "BRIDGE_MINT_WINDOW_SECONDS exceeds uint64");
        // forge-lint: disable-next-line(unsafe-typecast)
        cfg.mintWindowSeconds = uint64(windowSeconds); // range-checked above

        cfg.minBridgeOutE8s = _envUint("BRIDGE_MIN_BRIDGE_OUT_E8S");
    }

    /// @notice Reject any config or chain the script must not deploy to.
    function validateConfig(Config memory cfg) public view {
        // Chain guard. Each mainnet checks only its own flag.
        require(cfg.expectedChainId == block.chainid, "EXPECTED_CHAIN_ID does not match block.chainid");
        uint256 id = block.chainid;
        if (
            id == ETHEREUM_SEPOLIA_CHAIN_ID || id == ARBITRUM_SEPOLIA_CHAIN_ID || id == MONAD_TESTNET_CHAIN_ID
                || id == LOCAL_ANVIL_CHAIN_ID
        ) {
            // allowed
        } else if (id == ETHEREUM_MAINNET_CHAIN_ID) {
            require(cfg.allowEthereumMainnet, "Ethereum mainnet (1) requires ALLOW_ETHEREUM_MAINNET=true");
        } else if (id == ARBITRUM_ONE_CHAIN_ID) {
            require(cfg.allowArbitrumMainnet, "Arbitrum One (42161) requires ALLOW_ARBITRUM_MAINNET=true");
        } else if (id == MONAD_MAINNET_CHAIN_ID) {
            require(cfg.allowMonadMainnet, "Monad mainnet (143) requires ALLOW_MONAD_MAINNET=true");
        } else if (id == BASE_MAINNET_CHAIN_ID || id == BASE_SEPOLIA_CHAIN_ID) {
            revert("Base chains deploy with DeployBridgedIcUSD, not this script");
        } else {
            revert(
                "unsupported chain id (allowed: 11155111, 421614, 10143, 31337, and 1, 42161, 143 with their ALLOW_<CHAIN>_MAINNET flag)"
            );
        }

        // Role separation.
        require(cfg.admin != address(0), "admin is the zero address");
        require(cfg.minter != address(0), "minter is the zero address");
        require(cfg.admin != cfg.minter, "admin and minter must be different addresses");
        require(
            cfg.guardian != cfg.admin && cfg.guardian != cfg.minter, "guardian must be distinct from admin and minter"
        );

        // Economic parameters.
        require(cfg.mintWindowSeconds > 0, "mint window seconds is zero");
        require(cfg.supplyCapE8s > 0, "supply cap is zero");
        require(cfg.mintWindowLimitE8s > 0, "mint window limit is zero");
        require(cfg.minBridgeOutE8s > 0, "min bridge-out is zero");
    }

    /// @notice Validate, deploy under broadcast, then assert the deployed state.
    function deploy(Config memory cfg) internal returns (BridgedIcUSD token) {
        validateConfig(cfg);

        // Resolve the address that will actually sign (and so be msg.sender of the
        // constructor) with a transaction-free broadcast block, and refuse to go on
        // if it, the script's caller or tx.origin is a configured role holder. The
        // deployer must never be (or look like) a role holder, so it can never end up
        // with authority and the "no role" post-check is meaningful. Checking before
        // the real broadcast block leaves no broadcast context dangling on failure.
        vm.startBroadcast();
        (, address broadcaster,) = vm.readCallers();
        vm.stopBroadcast();
        _requireNotRoleHolder(broadcaster, cfg);
        _requireNotRoleHolder(msg.sender, cfg);
        _requireNotRoleHolder(tx.origin, cfg);

        vm.startBroadcast();
        token = new BridgedIcUSD(
            cfg.admin,
            cfg.adminDelaySeconds,
            cfg.minter,
            cfg.guardian,
            cfg.supplyCapE8s,
            cfg.mintWindowLimitE8s,
            cfg.mintWindowSeconds,
            cfg.minBridgeOutE8s
        );
        vm.stopBroadcast();

        verifyDeployment(token, cfg, broadcaster);

        console2.log("BridgedIcUSD deployed at:", address(token));
        console2.log("chain id:", block.chainid);
        console2.log("admin:", cfg.admin);
        console2.log("minter:", cfg.minter);
        console2.log("guardian:", cfg.guardian);
        console2.log("deployer (holds no role):", broadcaster);
    }

    /// @notice Post-deploy assertions. AccessControl is not enumerable, so "exactly
    /// the intended roles" is checked for every known account plus the role admins. The
    /// runtime code must equal the BridgedIcUSD artifact this script was compiled with,
    /// which is what "identical bytecode on every chain" means in practice.
    function verifyDeployment(BridgedIcUSD token, Config memory cfg, address deployer) public view {
        require(address(token).code.length > 0, "no code at deployed address");
        require(
            address(token).codehash == keccak256(type(BridgedIcUSD).runtimeCode),
            "runtime bytecode differs from BridgedIcUSD"
        );
        require(token.decimals() == 8, "decimals != 8");
        require(keccak256(bytes(token.VERSION())) == keccak256(bytes(EXPECTED_VERSION)), "VERSION mismatch");
        require(keccak256(bytes(token.name())) == keccak256("Rumi icUSD"), "name mismatch");
        require(keccak256(bytes(token.symbol())) == keccak256("icUSD"), "symbol mismatch");
        require(token.totalSupply() == 0, "initial supply != 0");

        // Configuration.
        require(token.supplyCap() == cfg.supplyCapE8s, "supplyCap mismatch");
        require(token.mintWindowLimit() == cfg.mintWindowLimitE8s, "mintWindowLimit mismatch");
        require(token.mintWindowSeconds() == cfg.mintWindowSeconds, "mintWindowSeconds mismatch");
        require(token.minBridgeOut() == cfg.minBridgeOutE8s, "minBridgeOut mismatch");
        require(token.mintedInWindow() == 0, "mintedInWindow != 0");
        require(token.bridgeOutCount() == 0, "bridgeOutCount != 0");
        require(!token.mintPaused() && !token.bridgeOutPaused(), "deployed paused");

        // Roles.
        bytes32 adminRole = token.DEFAULT_ADMIN_ROLE();
        bytes32 minterRole = token.MINTER_ROLE();
        bytes32 pauserRole = token.PAUSER_ROLE();

        require(token.defaultAdmin() == cfg.admin, "defaultAdmin mismatch");
        require(token.defaultAdminDelay() == cfg.adminDelaySeconds, "admin delay mismatch");
        (address pendingAdmin, uint48 pendingSchedule) = token.pendingDefaultAdmin();
        require(pendingAdmin == address(0) && pendingSchedule == 0, "unexpected pending admin transfer");
        require(token.getRoleAdmin(minterRole) == adminRole, "MINTER_ROLE admin changed");
        require(token.getRoleAdmin(pauserRole) == adminRole, "PAUSER_ROLE admin changed");

        require(token.hasRole(adminRole, cfg.admin), "admin lacks DEFAULT_ADMIN_ROLE");
        require(token.hasRole(pauserRole, cfg.admin), "admin lacks PAUSER_ROLE");
        require(!token.hasRole(minterRole, cfg.admin), "admin holds MINTER_ROLE");

        require(token.hasRole(minterRole, cfg.minter), "minter lacks MINTER_ROLE");
        require(token.hasRole(pauserRole, cfg.minter), "minter lacks PAUSER_ROLE");
        require(!token.hasRole(adminRole, cfg.minter), "minter holds DEFAULT_ADMIN_ROLE");

        if (cfg.guardian != address(0)) {
            require(token.hasRole(pauserRole, cfg.guardian), "guardian lacks PAUSER_ROLE");
            require(!token.hasRole(adminRole, cfg.guardian), "guardian holds DEFAULT_ADMIN_ROLE");
            require(!token.hasRole(minterRole, cfg.guardian), "guardian holds MINTER_ROLE");
        }

        require(!token.hasRole(adminRole, deployer), "deployer holds DEFAULT_ADMIN_ROLE");
        require(!token.hasRole(minterRole, deployer), "deployer holds MINTER_ROLE");
        require(!token.hasRole(pauserRole, deployer), "deployer holds PAUSER_ROLE");
    }

    function _requireNotRoleHolder(address who, Config memory cfg) internal pure {
        require(
            who != cfg.admin && who != cfg.minter && (cfg.guardian == address(0) || who != cfg.guardian),
            "deployer must not be the admin, minter or guardian"
        );
    }
}
