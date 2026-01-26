<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { walletStore } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import VaultDetails from '$lib/components/vault/VaultDetails.svelte';
  import { isDevelopment } from '$lib/config';
  import { permissionStore } from '$lib/stores/permissionStore';
  import type { EnhancedVault } from '$lib/services/types';
  import { vaultStore } from '$lib/stores/vaultStore';
  import { developerAccess } from '$lib/stores/developer';
  
  // Get vault ID from URL params - with proper number parsing
  $: vaultId = parseInt($page.params.id) || 0;
  
  // Track if vault was closed to prevent reload attempts
  let vaultWasClosed = false;
  
  // State management with proper types
  let vault: EnhancedVault | null = null;
  let isLoading = true;
  let error = '';
  let icpPrice = 0;
  
  // Developer mode management
  let showPasskeyInput = false;
  let passkey = "";
  let passkeyError = "";
  
  // Reactive declarations with proper typing
  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;
  $: canViewVaults = isDevelopment || $developerAccess || isConnected || ($permissionStore.initialized && $permissionStore.canViewVaults);
  
  // Debug logging for vault access
  $: if (typeof canViewVaults !== 'undefined') {
    console.log('🔍 Vault detail access check:', {
      isDevelopment,
      developerAccess: $developerAccess,
      isConnected,
      permissionStoreInitialized: $permissionStore.initialized,
      permissionStoreCanViewVaults: $permissionStore.canViewVaults,
      finalCanViewVaults: canViewVaults
    });
  }
  
  // Handle developer passkey submission
  function handlePasskeySubmit() {
    const isValid = developerAccess.checkPasskey(passkey);
    if (isValid) {
      passkeyError = '';
      passkey = '';
      showPasskeyInput = false;
      // Load vault now that we have access
      loadVault();
    } else {
      passkeyError = 'Invalid developer passkey';
    }
  }
  
  // Load vault data
  async function loadVault() {
    // Don't load if vault was closed - we're navigating away
    if (vaultWasClosed) {
      return;
    }
    
    // Check for vault access first - developer access can bypass wallet connection
    if (!canViewVaults) {
      error = 'Vault access required';
      isLoading = false;
      return;
    }
    
    // If not in developer mode, require wallet connection
    if (!$developerAccess && !isConnected) {
      goto('/');
      return;
    }
    
    isLoading = true;
    error = '';
    
    try {
      // First try to get vault from store
      vault = await vaultStore.getVault(vaultId);
      
      // If vault not in store, load from API
      if (!vault) {
        // Get protocol status for ICP price
        const status = await protocolService.getProtocolStatus();
        icpPrice = status.lastIcpRate;
        
        // In developer mode without wallet, show placeholder info
        if ($developerAccess && !isConnected) {
          // Create a demo vault for developer mode
          vault = {
            vaultId,
            owner: 'developer-mode-principal',
            icpMargin: 10.0, // 10 ICP collateral
            borrowedIcusd: 50.0, // 50 icUSD borrowed
            timestamp: Date.now(),
            lastUpdated: Date.now(),
            collateralRatio: 200.0, // 200% ratio
            collateralValueUSD: 10.0 * icpPrice,
            maxBorrowable: 75.0, // Can borrow up to 75 icUSD
            status: 'healthy' as const
          };
        } else {
          // Get user vaults and filter by ID
          const userVaults = await protocolService.getUserVaults();
          const foundVault = userVaults.find(v => v.vaultId === vaultId);
          
          if (foundVault) {
            // Enhance vault with calculated properties
            vault = vaultStore.enhanceVault(foundVault, icpPrice);
          } else {
            error = 'Vault not found or you do not have permission to access it';
          }
        }
      } else {
        // We already have the vault data from the store
        // Just get the latest ICP price for up-to-date calculations
        const status = await protocolService.getProtocolStatus();
        icpPrice = status.lastIcpRate;
      }
    } catch (err) {
      console.error('Error loading vault:', err);
      error = err instanceof Error ? err.message : 'Failed to load vault details';
    } finally {
      isLoading = false;
    }
  }
  
  // Reactive loading when access is granted - developer mode can bypass wallet connection
  // Don't reload if the vault was explicitly closed
  $: if (canViewVaults && ($developerAccess || isConnected) && !vault && !isLoading && !vaultWasClosed) {
    console.log('🚀 Access granted, loading vault...');
    loadVault();
  }
  
  // Handle vault close event from VaultDetails component
  function handleVaultClose(event: CustomEvent<{ vaultId: number }>) {
    console.log('🔒 Vault closed event received:', event.detail.vaultId);
    vaultWasClosed = true;
    // Redirect immediately to vaults list
    goto('/vaults');
  }

  onMount(() => {
    console.log('🚀 Vault detail page mounted for vault:', vaultId);
    // Load vault if we already have access - developer mode can bypass wallet connection
    if (canViewVaults && ($developerAccess || isConnected)) {
      loadVault();
    }
  });
</script>

<svelte:head>
  <title>RUMI Protocol - Vault #{vaultId}</title>
</svelte:head>

<div class="max-w-4xl mx-auto p-6">
  <div class="mb-8">
    <button 
      class="flex items-center text-gray-300 hover:text-white mb-4"
      on:click={() => goto('/vaults')}
    >
      <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 mr-1" viewBox="0 0 20 20" fill="currentColor">
        <path fill-rule="evenodd" d="M9.707 16.707a1 1 0 01-1.414 0l-6-6a1 1 0 010-1.414l6-6a1 1 0 011.414 1.414L5.414 9H17a1 1 0 110 2H5.414l4.293 4.293a1 1 0 010 1.414z" clip-rule="evenodd" />
      </svg>
      Back to Vaults
    </button>
  
    <h1 class="text-3xl font-bold mb-2">Vault #{vaultId}</h1>
    <p class="text-gray-400">Manage your collateral and debt position</p>
  </div>
  
  {#if !canViewVaults}
    <!-- Developer Access Required Section -->
    <div class="bg-gray-900/50 p-6 rounded-lg shadow-lg backdrop-blur-sm border border-purple-500/30">
      <div class="flex items-center gap-2 mb-4">
        <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6 text-purple-400" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 0 00-8 0v4h8z" />
        </svg>
        <h2 class="text-2xl font-semibold">Vault Access Required</h2>
      </div>
      
      <p class="text-gray-300 mb-6">
        Please connect your wallet to view vault details.
      </p>
      
      {#if showPasskeyInput}
        <div class="mb-4">
          <div class="flex gap-2">
            <input 
              type="password" 
              bind:value={passkey} 
              placeholder="Enter developer passkey"
              class="flex-grow p-2 bg-gray-800 rounded border border-gray-700 focus:outline-none focus:ring-2 focus:ring-purple-500"
              on:keydown={(e) => e.key === 'Enter' && handlePasskeySubmit()}
            />
            <button 
              class="px-4 py-2 bg-purple-600 hover:bg-purple-500 rounded-md"
              on:click={handlePasskeySubmit}
            >
              Submit
            </button>
          </div>
          {#if passkeyError}
            <p class="text-red-400 text-sm mt-2">{passkeyError}</p>
          {/if}
        </div>
      {:else}
        <button
          class="px-4 py-2 bg-purple-600 hover:bg-purple-500 rounded-md"
          on:click={() => showPasskeyInput = true}
        >
          Enter Developer Mode
        </button>
      {/if}
    </div>
  {:else if isLoading}
    <div class="flex justify-center p-12">
      <div class="w-8 h-8 border-4 border-purple-500 border-t-transparent rounded-full animate-spin"></div>
      <p class="ml-4 text-gray-400">Loading vault data...</p>
    </div>
  {:else if vaultWasClosed}
    <div class="p-4 bg-green-900/50 border border-green-500 rounded-lg text-green-200">
      <p>Vault successfully closed. Redirecting to vaults list...</p>
    </div>
  {:else if error}
    <div class="p-4 bg-red-900/50 border border-red-500 rounded-lg text-red-200">
      {error}
      
      <div class="mt-4">
        <button 
          class="px-4 py-2 bg-red-700 hover:bg-red-600 rounded text-white"
          on:click={() => goto('/vaults')}
        >
          Return to Vaults
        </button>
      </div>
    </div>
  {:else if vault}
    <!-- Beta Notice -->
    <div class="flex justify-end mb-4">
      <div class="bg-yellow-900/30 border border-yellow-600/50 px-3 py-1 rounded-full text-xs text-yellow-200 flex items-center gap-2">
        <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4 text-yellow-500" viewBox="0 0 20 20" fill="currentColor">
          <path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" />
        </svg>
        Beta
      </div>
    </div>
    
    <VaultDetails vaultId={vault.vaultId} icpPrice={icpPrice} on:close={handleVaultClose} />
  {/if}
</div>
