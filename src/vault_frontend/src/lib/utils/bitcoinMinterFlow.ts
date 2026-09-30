const MAX_NAT64 = 18_446_744_073_709_551_615n;
const SATS_PER_BTC = 100_000_000n;

export function parseBtcSats(raw: string): bigint | null {
  const match = /^(0|[1-9][0-9]*)(?:\.([0-9]{1,8}))?$/.exec(raw.trim());
  if (!match) return null;
  const sats = BigInt(match[1] + (match[2] ?? '').padEnd(8, '0'));
  return sats > 0n && sats <= MAX_NAT64 ? sats : null;
}

export function formatBtcSats(sats: bigint): string {
  const whole = sats / SATS_PER_BTC;
  const fraction = (sats % SATS_PER_BTC).toString().padStart(8, '0').replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

/** Bitcoin canister Candid blob Txid bytes are little endian; explorer hex is reversed. */
export function txidBytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes).reverse().map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

const BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

// Base58 validation is synchronous and verifies the four-byte Base58Check checksum.
function sha256(input: Uint8Array): Uint8Array {
  const K = SHA256_K;
  const bitLength = input.length * 8;
  const paddedLength = Math.ceil((input.length + 9) / 64) * 64;
  const data = new Uint8Array(paddedLength);
  data.set(input);
  data[input.length] = 0x80;
  const view = new DataView(data.buffer);
  view.setUint32(paddedLength - 4, bitLength >>> 0, false);
  view.setUint32(paddedLength - 8, Math.floor(bitLength / 0x1_0000_0000), false);
  const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
  const w = new Uint32Array(64);
  const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));
  for (let offset = 0; offset < data.length; offset += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4, false);
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15], b = w[i - 2];
      const s0 = rotr(a, 7) ^ rotr(a, 18) ^ (a >>> 3);
      const s1 = rotr(b, 17) ^ rotr(b, 19) ^ (b >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + s1 + ch + K[i] + w[i]) >>> 0;
      const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (s0 + maj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    [a, b, c, d, e, f, g, hh].forEach((value, i) => { h[i] = (h[i] + value) >>> 0; });
  }
  const output = new Uint8Array(32);
  const out = new DataView(output.buffer);
  h.forEach((value, i) => out.setUint32(i * 4, value, false));
  return output;
}

const SHA256_K = new Uint32Array([
  0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
  0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
  0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,
  0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
]);

function doubleSha256(input: Uint8Array): Uint8Array {
  return sha256(sha256(input));
}

function validBase58Check(value: string): boolean {
  const bytes = decodeBase58Bytes(value);
  if (!bytes || bytes.length !== 25 || (bytes[0] !== 0x00 && bytes[0] !== 0x05)) return false;
  const payload = Uint8Array.from(bytes.slice(0, 21));
  const checksum = doubleSha256(payload).slice(0, 4);
  return checksum.every((byte, index) => byte === bytes[index + 21]);
}

function decodeBase58Bytes(value: string): number[] | null {
  let number = 0n;
  for (const char of value) {
    const digit = BASE58.indexOf(char);
    if (digit < 0) return null;
    number = number * 58n + BigInt(digit);
  }
  const bytes: number[] = [];
  while (number > 0n) { bytes.unshift(Number(number & 255n)); number >>= 8n; }
  for (const char of value) { if (char !== '1') break; bytes.unshift(0); }
  return bytes;
}

const BECH32_CHARS = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
function bech32Polymod(values: number[]): number {
  const generators = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let chk = 1;
  for (const value of values) {
    const top = chk >>> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ value;
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= generators[i];
  }
  return chk >>> 0;
}

function validSegwitAddress(value: string): boolean {
  if (value.length < 14 || value.length > 90 || value !== value.toLowerCase()) return false;
  const separator = value.lastIndexOf('1');
  if (separator < 1 || separator + 7 > value.length || value.slice(0, separator) !== 'bc') return false;
  const data = [...value.slice(separator + 1)].map((char) => BECH32_CHARS.indexOf(char));
  if (data.some((char) => char < 0)) return false;
  const version = data[0];
  if (version < 0 || version > 16) return false;
  const hrp = [...'bc'].map((char) => char.charCodeAt(0) >> 5);
  hrp.push(0);
  for (const char of 'bc') hrp.push(char.charCodeAt(0) & 31);
  const polymod = bech32Polymod([...hrp, ...data]);
  const witnessValues = data.slice(1, -6);
  let accumulator = 0;
  let bits = 0;
  const program: number[] = [];
  for (const value of witnessValues) {
    accumulator = ((accumulator << 5) | value) & 0xfff;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      program.push((accumulator >> bits) & 0xff);
    }
  }
  if (bits >= 5 || ((accumulator << (8 - bits)) & 0xff) !== 0) return false;
  if (program.length < 2 || program.length > 40) return false;
  const encoding = polymod === 1 ? 'bech32' : polymod === 0x2bc830a3 ? 'bech32m' : null;
  if (!encoding || (version === 0 && encoding !== 'bech32') || (version > 0 && encoding !== 'bech32m')) return false;
  return version !== 0 || program.length === 20 || program.length === 32;
}

export function isValidBitcoinMainnetAddress(raw: string): boolean {
  const value = raw.trim();
  if (!value) return false;
  if (value.toLowerCase().startsWith('bc1')) {
    if (value !== value.toLowerCase() && value !== value.toUpperCase()) return false;
    return validSegwitAddress(value.toLowerCase());
  }
  return validBase58Check(value);
}
