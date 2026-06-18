// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";

/// @title IcUSD - Rumi icUSD on Monad
/// @notice Canister-minted ERC-20. The Rumi backend canister's tECDSA-derived
/// settlement address holds MINTER_ROLE and is the sole minter. Any holder may
/// burn to repay their vault. 8 decimals so 1 base unit == 1 e8s (1:1 with the
/// ICP-side e8s accounting). totalSupply() reflects Monad-side circulation only;
/// the canonical all-chains total is get_global_icusd_supply() on the Rumi backend.
contract IcUSD is ERC20, AccessControl {
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");

    /// @notice vault_id + recipient/burner are INDEXED so the Rumi observer reads
    /// them from topics[1]/topics[2] and the amount from data. The topic0 hashes
    /// are pinned in the backend (MINT_EVENT_TOPIC0 / BURN_EVENT_TOPIC0).
    event Mint(uint256 indexed vault_id, address indexed recipient, uint256 amount);
    event Burn(uint256 indexed vault_id, address indexed burner, uint256 amount);

    /// @dev One mint per settlement op_id (idempotency guard): a canister
    /// resubmit-after-transient-RPC-error must not double-mint on-chain. Keyed
    /// per-OP (not per-vault) so a vault can be minted to more than once across
    /// distinct ops (borrow). `op_id` is the Rumi settlement queue's
    /// unique-per-chain op id.
    mapping(uint64 => bool) public mintedOps;

    constructor(address admin, address minter) ERC20("Rumi icUSD", "icUSD") {
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(MINTER_ROLE, minter);
    }

    function decimals() public pure override returns (uint8) {
        return 8;
    }

    /// @notice Mint icUSD. Only the canister settlement address (MINTER_ROLE).
    /// `op_id` is the settlement op's unique id; reverts if it was already minted
    /// (per-op idempotency). `vault_id` stays the debt key carried in the `Mint`
    /// event and used by `burn` (repay), so a vault can be minted to more than
    /// once via distinct `op_id`s (borrow). The `Mint` event is UNCHANGED so the
    /// backend log decoder (`MintLog`) is unaffected.
    function mint(address to, uint256 amount, uint64 vault_id, uint64 op_id) external onlyRole(MINTER_ROLE) {
        require(!mintedOps[op_id], "op already minted");
        mintedOps[op_id] = true;
        _mint(to, amount);
        emit Mint(uint256(vault_id), to, amount);
    }

    /// @notice Burn icUSD to repay vault `target_vault_id`. Callable by anyone
    /// holding the tokens; the Rumi backend observes the Burn event and
    /// decrements the vault's debt + the Monad chain supply.
    function burn(uint256 amount, uint64 target_vault_id) external {
        _burn(msg.sender, amount);
        emit Burn(uint256(target_vault_id), msg.sender, amount);
    }
}
