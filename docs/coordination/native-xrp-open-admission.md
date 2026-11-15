# Native-XRP open admission

Public permissionless native-XRP opens are disabled. New opens require a
developer-issued, one-time principal permit. The permit is consumed atomically
before reserve reads or threshold address derivation. A failure before an address
is returned restores the permit unless the developer revoked it while the call
was in flight.

Existing vaults, XRP claims, and pending custody addresses remain usable. Cancelling
or sweeping a pending open marks a durable tombstone; it never deletes the owner,
derivation nonce, or XRPL custody address. A late deposit can still be confirmed,
and reopening by the same owner reactivates the original address. Tombstones do not
consume active admission capacity.

Permissionless opens must remain disabled until a scarce-cost admission design
exists and has been implemented and reviewed. An empty permit set is the default
for new installs and upgrades from older snapshots.
