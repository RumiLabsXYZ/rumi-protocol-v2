import { afterEach, describe, expect, it, vi } from 'vitest';
import { createCksolReadPolling } from './cksolReadPolling';

afterEach(() => vi.useRealTimers());
describe('read-only ckSOL polling', () => {
  it('backs off unchanged waiting reads, resets for progress, and stops on completion', async () => {
    vi.useFakeTimers();
    let active = true, state = 'waiting';
    const read = vi.fn(async () => {});
    const poll = createCksolReadPolling({ read, active: () => active, fingerprint: () => state, visible: () => true });
    poll.sync();
    await vi.advanceTimersByTimeAsync(15_000); expect(read).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(29_999); expect(read).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1); expect(read).toHaveBeenCalledTimes(2);
    state = 'deposit detected';
    await vi.advanceTimersByTimeAsync(60_000); expect(read).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(15_000); expect(read).toHaveBeenCalledTimes(4);
    active = false; poll.sync();
    await vi.advanceTimersByTimeAsync(180_000); expect(read).toHaveBeenCalledTimes(4);
    await poll.checkAgain(); expect(read).toHaveBeenCalledTimes(5);
    poll.sync(); await vi.advanceTimersByTimeAsync(180_000); expect(read).toHaveBeenCalledTimes(5);
    active = true; state = 'new deposit'; poll.sync();
    await vi.advanceTimersByTimeAsync(15_000); expect(read).toHaveBeenCalledTimes(6);
    poll.destroy();
  });
  it('never overlaps reads, cancels hidden-tab timers and cannot restart after teardown', async () => {
    vi.useFakeTimers(); let visible = true;
    let finish!: () => void;
    const read = vi.fn(() => new Promise<void>(resolve => finish = resolve));
    const poll = createCksolReadPolling({ read, active: () => true, fingerprint: () => 'waiting', visible: () => visible });
    poll.sync(); await vi.advanceTimersByTimeAsync(15_000);
    void poll.checkAgain(); expect(read).toHaveBeenCalledTimes(1);
    visible = false; poll.sync(); finish(); await Promise.resolve();
    await vi.advanceTimersByTimeAsync(180_000); expect(read).toHaveBeenCalledTimes(1);
    visible = true; poll.sync(); await vi.advanceTimersByTimeAsync(30_000); expect(read).toHaveBeenCalledTimes(2);
    poll.destroy(); finish(); await Promise.resolve();
    poll.sync(); await poll.checkAgain(); await vi.advanceTimersByTimeAsync(180_000); expect(read).toHaveBeenCalledTimes(2);
  });
  it('cancels a queued manual read if its session is destroyed before the microtask', async () => {
    const read=vi.fn(async()=>{});
    const poll=createCksolReadPolling({read,active:()=>true,fingerprint:()=>'',visible:()=>true});
    const pending=poll.checkAgain();poll.destroy();await pending;
    expect(read).not.toHaveBeenCalled();
  });
  it('resets backed-off timers immediately when an external receipt makes progress', async () => {
    vi.useFakeTimers();let state='waiting';const read=vi.fn(async()=>{});
    const poll=createCksolReadPolling({read,active:()=>true,fingerprint:()=>state,visible:()=>true});
    poll.sync();await vi.advanceTimersByTimeAsync(45_000);expect(read).toHaveBeenCalledTimes(2);
    state='receipt advanced';poll.sync();await vi.advanceTimersByTimeAsync(15_000);
    expect(read).toHaveBeenCalledTimes(3);poll.destroy();
  });

});
