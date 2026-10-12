/** Every ckERC20 redemption uses the same owner's ckETH allowance. */
export function ckErc20WithdrawalLockName(ownerText: string): string {
	return `rumi:ckerc20:withdrawal:${ownerText}`;
}

export type CkErc20LockManager = {
	request<T>(
		name: string,
		options: { mode: 'exclusive'; ifAvailable: true },
		callback: (lock: unknown | null) => Promise<T>
	): Promise<T>;
};

export function withCkErc20WithdrawalLock<T>(
	locks: CkErc20LockManager,
	ownerText: string,
	callback: (lock: unknown | null) => Promise<T>
): Promise<T> {
	return locks.request(
		ckErc20WithdrawalLockName(ownerText),
		{ mode: 'exclusive', ifAvailable: true },
		callback
	);
}
