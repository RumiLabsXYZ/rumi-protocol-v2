// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {
    AccessControlDefaultAdminRules
} from "@openzeppelin/contracts/access/extensions/AccessControlDefaultAdminRules.sol";

/// @title BridgedIcUSD - Rumi icUSD on Base (escrow-backed bridge token)
///
/// @notice Base-side representation of the canonical icUSD that lives on the
/// Internet Computer (ICP) ledger. This token is ESCROW-BACKED, not debt-backed:
/// every unit in circulation on Base is matched 1:1 by ICP icUSD that a dedicated
/// ICP bridge canister holds in escrow. It is a separate, explicitly versioned
/// contract from the Monad/Conflux `IcUSD` (which is debt-backed and burns to
/// repay vaults). There is NO repayment burn here: the only way to destroy tokens
/// is `bridgeOut`, which is the bridge's redemption path back to ICP.
///
/// Flow:
///  - ICP -> Base: the bridge canister locks ICP icUSD in escrow, then calls
///    `mint(to, amount, mintId)` from its threshold-ECDSA minter address. `mintId`
///    is a unique id chosen by the canister so a resubmit after a transient RPC
///    error cannot double-mint (destination idempotency).
///  - Base -> ICP: a holder calls `bridgeOut(amount, icpPrincipal, icpSubaccount,
///    clientNonce)`. The tokens are burned and a `BridgeOut` event binds the amount,
///    the destination ICP account and a unique `transferId`. The canister observes
///    the event at Base L1-batch finality and releases the escrowed ICP icUSD.
///
/// 8 decimals, so 1 base unit == 1 ICP e8s. Both `mint` and `bridgeOut` amounts are
/// capped at `type(uint64).max` because the ICP side accounts in u64 e8s.
///
/// Authority model (who can do what):
///  - `MINTER_ROLE` is the ONLY role that can mint, and it is held by the bridge
///    canister's tECDSA minter address. There is no other mint path.
///  - `DEFAULT_ADMIN_ROLE` is held by the bridge canister's tECDSA admin address
///    (distinct from the minter). It can change the supply cap, the mint rate
///    limit and the minimum bridge-out amount, unpause, and grant or revoke any
///    non-admin role (including `MINTER_ROLE`). The admin is therefore a FULL MINT
///    AUTHORITY path (it can grant itself `MINTER_ROLE`) and must only ever be held
///    by the bridge canister. Admin transfer is the inherited OpenZeppelin two-step
///    delayed process (`beginDefaultAdminTransfer`, then `acceptDefaultAdminTransfer`
///    by the new admin after `defaultAdminDelay`); there is no other admin path.
///    A transfer moves only `DEFAULT_ADMIN_ROLE`: the old admin keeps any
///    `PAUSER_ROLE` it holds and the new admin does not receive one until granted.
///  - `PAUSER_ROLE` can only pause (`pauseMint`, `pauseBridgeOut`). It is granted to
///    the admin, the minter and an optional guardian. The guardian holds NO other
///    role. Only `DEFAULT_ADMIN_ROLE` can unpause.
///  - The deployer keeps no authority: the constructor grants nothing to
///    `msg.sender` unless the deployer is explicitly passed as `admin`.
///
/// Pause semantics: pausing mint or bridge-out never blocks plain ERC-20
/// `transfer`, `approve` or `transferFrom`.
contract BridgedIcUSD is ERC20, AccessControlDefaultAdminRules {
    /// @notice Contract version tag, pinned by off-chain decoders.
    string public constant VERSION = "rumi-base-icusd-bridge-v1";

    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");

    /// @notice Domain separator mixed into every `transferId`.
    bytes32 public constant BRIDGE_OUT_DOMAIN = keccak256("rumi.icusd.base.bridge-out.v1");

    /// @notice Maximum byte length of an ICP principal.
    uint256 public constant ICP_PRINCIPAL_MAX_LEN = 29;

    /// @notice Maximum `totalSupply()` allowed after a mint. Lowering it below the
    /// current `totalSupply()` is allowed: it only blocks further mints (it never
    /// burns or freezes existing balances and never blocks `bridgeOut`).
    uint256 public supplyCap;

    /// @notice Maximum amount that may be minted per fixed window.
    uint256 public mintWindowLimit;

    /// @notice Window length in seconds (always > 0).
    uint64 public mintWindowSeconds;

    /// @notice `block.timestamp` at which the current mint window began.
    uint64 public mintWindowStart;

    /// @notice Amount minted so far in the current window.
    uint256 public mintedInWindow;

    /// @notice Cumulative amount ever minted by `mint`. It never decreases (burns do not
    /// reduce it), so `totalSupply() == totalMinted - total burned`. The bridge canister
    /// compares it with the total it minted itself to detect a mint it did not make, even
    /// one that was burned again before the next supply check could see it.
    uint256 public totalMinted;

    /// @notice Minimum `bridgeOut` amount (a zero amount is rejected separately).
    uint256 public minBridgeOut;

    bool public mintPaused;
    bool public bridgeOutPaused;

    /// @notice Number of successful `bridgeOut` calls. Also the per-event sequence
    /// (the first `BridgeOut` carries sequence 1).
    uint64 public bridgeOutCount;

    /// @notice mintId -> already used (destination-side mint idempotency).
    mapping(bytes32 => bool) public mintConsumed;

    /// @notice transferId -> already used.
    mapping(bytes32 => bool) public bridgeOutUsed;

    /// @notice Emitted after the ERC-20 `Transfer` for every bridge mint.
    /// topic1 = mintId, topic2 = recipient, data = abi.encode(amount).
    event BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount);

    /// @notice Emitted for every bridge-out burn.
    /// topic1 = transferId, topic2 = from, data = abi.encode(amount, sequence,
    /// icpPrincipal, icpSubaccount).
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

    error MintIsPaused();
    error BridgeOutIsPaused();
    error MintAlreadyConsumed(bytes32 mintId);
    error SupplyCapExceeded(uint256 newSupply, uint256 cap);
    error MintRateLimitExceeded(uint256 requested, uint256 remaining);
    error InvalidRecipient(address to);
    error ZeroAmount();
    error AmountBelowMinimum(uint256 amount, uint256 minimum);
    error AmountTooLarge(uint256 amount);
    error InvalidIcpPrincipal();
    error BridgeOutAlreadyUsed(bytes32 transferId);
    error InvalidConfig();

    /// @param admin Holder of `DEFAULT_ADMIN_ROLE` (the bridge canister's tECDSA admin address).
    /// @param initialAdminDelay Seconds between `beginDefaultAdminTransfer` and a valid accept.
    /// @param minter Holder of `MINTER_ROLE` (the bridge canister's tECDSA minter address).
    /// @param guardian Optional pause-only account (zero address for none).
    /// @param supplyCap_ Maximum `totalSupply()` after a mint.
    /// @param mintWindowLimit_ Maximum amount mintable per window.
    /// @param mintWindowSeconds_ Window length in seconds (> 0).
    /// @param minBridgeOut_ Minimum `bridgeOut` amount.
    constructor(
        address admin,
        uint48 initialAdminDelay,
        address minter,
        address guardian,
        uint256 supplyCap_,
        uint256 mintWindowLimit_,
        uint64 mintWindowSeconds_,
        uint256 minBridgeOut_
    ) ERC20("Rumi icUSD", "icUSD") AccessControlDefaultAdminRules(initialAdminDelay, _nonZeroAdmin(admin)) {
        if (
            minter == address(0) || minter == admin || mintWindowSeconds_ == 0
                || (guardian != address(0) && (guardian == admin || guardian == minter))
        ) {
            revert InvalidConfig();
        }

        _grantRole(MINTER_ROLE, minter);
        _grantRole(PAUSER_ROLE, admin);
        _grantRole(PAUSER_ROLE, minter);
        if (guardian != address(0)) {
            _grantRole(PAUSER_ROLE, guardian);
        }

        supplyCap = supplyCap_;
        mintWindowLimit = mintWindowLimit_;
        mintWindowSeconds = mintWindowSeconds_;
        mintWindowStart = uint64(block.timestamp);
        minBridgeOut = minBridgeOut_;

        emit SupplyCapSet(supplyCap_);
        emit MintRateLimitSet(mintWindowLimit_, mintWindowSeconds_);
        emit MinBridgeOutSet(minBridgeOut_);
    }

    /// @dev Evaluated before the OpenZeppelin base constructor so a zero admin
    /// reverts with `InvalidConfig` like every other bad constructor argument.
    function _nonZeroAdmin(address admin) private pure returns (address) {
        if (admin == address(0)) revert InvalidConfig();
        return admin;
    }

    /// @notice 8 decimals so 1 base unit == 1 ICP e8s.
    function decimals() public pure override returns (uint8) {
        return 8;
    }

    // ------------------------------------------------------------------
    // ICP -> Base
    // ------------------------------------------------------------------

    /// @notice Mint bridged icUSD. Only `MINTER_ROLE` (the bridge canister's minter
    /// address). Each `mintId` can be used exactly once. Subject to the supply cap
    /// and the fixed-window mint rate limit.
    function mint(address to, uint256 amount, bytes32 mintId) external onlyRole(MINTER_ROLE) {
        if (mintPaused) revert MintIsPaused();
        if (to == address(0) || to == address(this)) revert InvalidRecipient(to);
        if (amount == 0) revert ZeroAmount();
        if (amount > type(uint64).max) revert AmountTooLarge(amount);
        if (mintConsumed[mintId]) revert MintAlreadyConsumed(mintId);

        uint256 newSupply = totalSupply() + amount;
        if (newSupply > supplyCap) revert SupplyCapExceeded(newSupply, supplyCap);

        // Fixed window: the first mint at or after the window end opens a new one.
        // Widened to uint256 so an enormous admin-set window cannot overflow.
        // forge-lint: disable-next-line(block-timestamp)
        if (block.timestamp >= uint256(mintWindowStart) + uint256(mintWindowSeconds)) {
            mintWindowStart = uint64(block.timestamp);
            mintedInWindow = 0;
        }
        uint256 used = mintedInWindow;
        uint256 limit = mintWindowLimit;
        if (used + amount > limit) {
            revert MintRateLimitExceeded(amount, used >= limit ? 0 : limit - used);
        }

        mintConsumed[mintId] = true;
        mintedInWindow = used + amount;
        totalMinted += amount;
        _mint(to, amount);
        emit BridgeMint(mintId, to, amount);
    }

    // ------------------------------------------------------------------
    // Base -> ICP
    // ------------------------------------------------------------------

    /// @notice Burn `amount` of the caller's tokens and request release of the same
    /// amount of escrowed ICP icUSD to the ICP account (`icpPrincipal`,
    /// `icpSubaccount`). This is the ONLY burn path. `clientNonce` makes the
    /// `transferId` unique per caller; reusing a nonce with the same sender reverts.
    /// `icpPrincipal` is the raw principal bytes (1 to 29 bytes). The anonymous
    /// principal (`0x04`) and the empty management-canister principal are rejected.
    /// A zero `icpSubaccount` means the default subaccount.
    function bridgeOut(uint256 amount, bytes calldata icpPrincipal, bytes32 icpSubaccount, bytes32 clientNonce)
        external
        returns (bytes32 transferId)
    {
        if (bridgeOutPaused) revert BridgeOutIsPaused();
        if (amount == 0) revert ZeroAmount();
        if (amount < minBridgeOut) revert AmountBelowMinimum(amount, minBridgeOut);
        if (amount > type(uint64).max) revert AmountTooLarge(amount);

        uint256 principalLen = icpPrincipal.length;
        if (principalLen == 0 || principalLen > ICP_PRINCIPAL_MAX_LEN) revert InvalidIcpPrincipal();
        if (principalLen == 1 && icpPrincipal[0] == 0x04) revert InvalidIcpPrincipal();

        transferId = computeTransferId(msg.sender, clientNonce);
        if (bridgeOutUsed[transferId]) revert BridgeOutAlreadyUsed(transferId);
        bridgeOutUsed[transferId] = true;

        _burn(msg.sender, amount);

        uint64 sequence = ++bridgeOutCount;
        emit BridgeOut(transferId, msg.sender, amount, sequence, icpPrincipal, icpSubaccount);
    }

    /// @notice The transfer id `bridgeOut` assigns to (`from`, `clientNonce`) on this
    /// chain and contract: keccak256(abi.encode(BRIDGE_OUT_DOMAIN, chainid, this,
    /// from, clientNonce)).
    function computeTransferId(address from, bytes32 clientNonce) public view returns (bytes32) {
        return keccak256(abi.encode(BRIDGE_OUT_DOMAIN, block.chainid, address(this), from, clientNonce));
    }

    // ------------------------------------------------------------------
    // Pause controls
    // ------------------------------------------------------------------

    /// @notice Stop all minting. Pause-only roles may call this; only the admin can undo it.
    function pauseMint() external onlyRole(PAUSER_ROLE) {
        mintPaused = true;
        emit MintPausedSet(true);
    }

    function unpauseMint() external onlyRole(DEFAULT_ADMIN_ROLE) {
        mintPaused = false;
        emit MintPausedSet(false);
    }

    /// @notice Stop all `bridgeOut` calls. Plain ERC-20 transfers are unaffected.
    function pauseBridgeOut() external onlyRole(PAUSER_ROLE) {
        bridgeOutPaused = true;
        emit BridgeOutPausedSet(true);
    }

    function unpauseBridgeOut() external onlyRole(DEFAULT_ADMIN_ROLE) {
        bridgeOutPaused = false;
        emit BridgeOutPausedSet(false);
    }

    // ------------------------------------------------------------------
    // Admin configuration
    // ------------------------------------------------------------------

    /// @notice Set the maximum `totalSupply()` after a mint. A value below the current
    /// `totalSupply()` is allowed and only blocks further mints.
    function setSupplyCap(uint256 cap) external onlyRole(DEFAULT_ADMIN_ROLE) {
        supplyCap = cap;
        emit SupplyCapSet(cap);
    }

    /// @notice Set the per-window mint limit and window length (> 0). The current
    /// window keeps its start time and `mintedInWindow`, so lowering the limit below
    /// what was already minted blocks mints until the window ends.
    function setMintRateLimit(uint256 limit, uint64 windowSeconds) external onlyRole(DEFAULT_ADMIN_ROLE) {
        if (windowSeconds == 0) revert InvalidConfig();
        mintWindowLimit = limit;
        mintWindowSeconds = windowSeconds;
        emit MintRateLimitSet(limit, windowSeconds);
    }

    function setMinBridgeOut(uint256 minimum) external onlyRole(DEFAULT_ADMIN_ROLE) {
        minBridgeOut = minimum;
        emit MinBridgeOutSet(minimum);
    }
}
