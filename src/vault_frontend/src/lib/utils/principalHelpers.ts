import { Principal } from '@dfinity/principal';

/**
 * Safe wrapper to handle Principal compatibility issues between different
 * versions of @dfinity/principal that might be loaded in the application.
 */
/**
 * Truncate a principal for display: first 5 chars + "…" + last 3 chars
 */
export function truncatePrincipal(principal: string | null | undefined): string {
  if (!principal) return '';
  const str = principal.toString();
  if (str.length <= 10) return str;
  return `${str.slice(0, 5)}…${str.slice(-3)}`;
}

/**
 * Copy text to clipboard and return success status
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // Fallback for older browsers
    try {
      const textarea = document.createElement('textarea');
      textarea.value = text;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
      return true;
    } catch {
      return false;
    }
  }
}

export class PrincipalUtils {
  /**
   * Creates a Principal from a string or another Principal object
   * This ensures that we're always using a consistent Principal instance
   */
  static fromText(text: string | Principal): Principal {
    if (typeof text === 'string') {
      return Principal.fromText(text);
    }
    // If it's already a Principal, convert to string and back to ensure consistent version
    return Principal.fromText(text.toString());
  }
  
  /**
   * Safely converts a Principal to an array representation suitable for canister calls
   */
  static toCanisterParam(principal: Principal | string | null | undefined): Principal {
    if (!principal) return Principal.anonymous();
    
    // Convert to string first, then to Principal to ensure we're using the same implementation
    const principalText = principal.toString();
    return Principal.fromText(principalText);
  }
  
  /**
   * Creates an array of Principal objects for canister calls
   */
  static principalListParam(principals: (Principal | string)[]): Principal[] {
    return principals.map(p => this.toCanisterParam(p));
  }
  
  /**
   * Creates a compatible principal object that can be used across different versions
   * of the Principal class. This is useful for canister calls that might encounter
   * compatibility issues.
   */
  static createCompatiblePrincipal(principal: Principal | string | null | undefined): any {
    if (!principal) return Principal.anonymous();
    
    const principalText = principal.toString();
    
    // For maximum compatibility, return a string that the backend can parse
    // This bypasses Principal type checking entirely
    return principalText;
  }
}
