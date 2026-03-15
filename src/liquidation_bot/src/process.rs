use crate::state;
use ic_canister_log::log;

pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };
    log!(crate::INFO, "Processing vault #{} (stub — DEX swap not yet implemented)", vault.vault_id);
    // TODO: Task 3 will implement the full flow
}
