import { writable } from 'svelte/store';

function createDeveloperAccessStore() {
  // Beta mode: Always grant access to everyone
  const { subscribe, set } = writable(true);
  
  return {
    subscribe,
    
    // Legacy method - now always returns true for beta
    checkPasskey(passkey: string): boolean {
      return true;
    },
    
    // Check if developer access is stored in session - always true for beta
    checkStoredAccess(): boolean {
      set(true);
      return true;
    },
    
    // Clear developer access - no-op for beta since access is always granted
    clearAccess() {
      // In beta mode, we don't actually revoke access
      // This is kept for API compatibility
    }
  };
}

export const developerAccess = createDeveloperAccessStore();

// In beta mode, access is always granted
if (typeof window !== 'undefined') {
  setTimeout(() => {
    developerAccess.checkStoredAccess();
  }, 0);
}
