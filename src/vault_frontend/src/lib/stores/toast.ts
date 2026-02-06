import { writable } from 'svelte/store';

export interface ToastData {
  id: string;
  message: string;
  type: 'success' | 'error' | 'info';
  duration: number;
}

function createToastStore() {
  const { subscribe, update } = writable<ToastData[]>([]);
  
  function addToast(message: string, type: 'success' | 'error' | 'info' = 'success', duration: number = 2000): string {
    const id = crypto.randomUUID ? crypto.randomUUID() : Math.random().toString(36).substring(2);
    const toast: ToastData = { id, message, type, duration };
    
    update(toasts => [...toasts, toast]);
    
    return id;
  }
  
  function removeToast(id: string) {
    update(toasts => toasts.filter(t => t.id !== id));
  }
  
  return {
    subscribe,
    success: (message: string, duration?: number) => addToast(message, 'success', duration),
    error: (message: string, duration?: number) => addToast(message, 'error', duration),
    info: (message: string, duration?: number) => addToast(message, 'info', duration),
    remove: removeToast
  };
}

export const toastStore = createToastStore();
