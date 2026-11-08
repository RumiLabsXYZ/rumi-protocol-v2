//! Local anvil (EVM) process management and a blocking JSON-RPC client.
//!
//! Everything here is LOCAL: anvil binds 127.0.0.1 on a free port and is killed
//! when the [`Anvil`] handle is dropped. The client is a tiny HTTP/1.1 POST over
//! `std::net::TcpStream` (no HTTP crate), so the harness adds no dependencies.
//!
//! # What `finalized` and `safe` mean on anvil
//!
//! Anvil has no consensus layer. It emulates the `safe` and `finalized` block
//! tags from the configured `--slots-in-an-epoch` (default 32):
//!
//! ```text
//! safe      = latest - slots_in_an_epoch        (saturating at 0)
//! finalized = latest - 2 * slots_in_an_epoch    (saturating at 0)
//! ```
//!
//! This harness starts anvil with `--slots-in-an-epoch 1`, so
//! `safe = latest - 1` and `finalized = latest - 2`. The smoke test
//! (`harness_smoke_pic.rs`, test `anvil_boots_and_finalized_tag_semantics`)
//! measures this empirically and asserts it, so a future anvil release that
//! changes the formula fails loudly instead of silently shifting finality.
//!
//! A transaction mined into block N therefore becomes visible to a
//! `finalized`-tagged read only once the chain has advanced to latest >= N + 2.
//! Scenario tests that need "wait for finality" call [`Anvil::mine`] with 2.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Chain id every harness anvil runs with (0x7a69).
pub const ANVIL_CHAIN_ID: u64 = 31337;

/// Path of the anvil binary: `$ANVIL_BIN`, else `$HOME/.foundry/bin/anvil`.
pub fn anvil_bin() -> String {
    if let Ok(p) = std::env::var("ANVIL_BIN") {
        return p;
    }
    let home = std::env::var("HOME").expect("HOME is not set and ANVIL_BIN is not set");
    format!("{home}/.foundry/bin/anvil")
}

/// A cloneable handle to an anvil HTTP endpoint. The outcall router holds one of
/// these so it can forward request bodies upstream while the test still owns the
/// [`Anvil`] process handle.
#[derive(Clone, Debug)]
pub struct Endpoint {
    addr: SocketAddr,
}

impl Endpoint {
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// POST `body` (a JSON-RPC request, forwarded byte-for-byte) and return
    /// `(http_status, response_body)`.
    pub fn post(&self, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
        let mut stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5))
            .map_err(|e| format!("connect {}: {e}", self.addr))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let head = format!(
            "POST / HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr,
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(body))
            .and_then(|_| stream.flush())
            .map_err(|e| format!("write: {e}"))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| format!("read: {e}"))?;
        parse_http_response(&raw)
    }

    /// Send one JSON-RPC call; `Ok(result)` or `Err(error object)`.
    pub fn try_rpc(&self, method: &str, params: Value) -> Result<Value, Value> {
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (status, body) = self
            .post(req.to_string().as_bytes())
            .map_err(|e| json!({"transport": e}))?;
        let v: Value = serde_json::from_slice(&body)
            .map_err(|e| json!({"status": status, "parse": e.to_string(), "body": String::from_utf8_lossy(&body)}))?;
        if let Some(err) = v.get("error") {
            return Err(err.clone());
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| json!({"status": status, "no_result": v}))
    }
}

/// Parse a raw HTTP/1.1 response (read until the server closed the socket).
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("no header terminator in {} bytes", raw.len()))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|e| e.to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or("empty response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("bad status line: {status_line}"))?
        .parse()
        .map_err(|e| format!("bad status code in {status_line}: {e}"))?;
    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("transfer-encoding:") {
            chunked = v.trim().contains("chunked");
        } else if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let body = &raw[split + 4..];
    let body = if chunked {
        decode_chunked(body)?
    } else if let Some(n) = content_length {
        if body.len() < n {
            return Err(format!("truncated body: have {} want {n}", body.len()));
        }
        body[..n].to_vec()
    } else {
        body.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("chunk size line not terminated")?;
        let size_str = std::str::from_utf8(&data[..eol]).map_err(|e| e.to_string())?;
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|e| format!("chunk size {size_str:?}: {e}"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err("truncated chunk".to_string());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// A running local anvil node. Killed on drop.
pub struct Anvil {
    child: Child,
    endpoint: Endpoint,
    /// anvil's unlocked default accounts (`eth_accounts`), lowercase 0x-prefixed.
    pub accounts: Vec<String>,
}

impl Anvil {
    /// Spawn anvil on a free localhost port with `--chain-id 31337
    /// --slots-in-an-epoch 1 --silent` and wait until it answers `eth_chainId`.
    pub fn start() -> Anvil {
        let bin = anvil_bin();
        // Pick a free port. There is a tiny window between dropping the listener
        // and anvil binding; retry with a new port if anvil fails to come up.
        let mut last_err = String::new();
        for _attempt in 0..5 {
            let port = {
                let l = TcpListener::bind("127.0.0.1:0").expect("bind probe port");
                l.local_addr().unwrap().port()
            };
            let mut child = Command::new(&bin)
                .args([
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--chain-id",
                    &ANVIL_CHAIN_ID.to_string(),
                    "--slots-in-an-epoch",
                    "1",
                    "--silent",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap_or_else(|e| panic!("failed to spawn anvil at {bin}: {e} (set ANVIL_BIN)"));
            let endpoint = Endpoint {
                addr: SocketAddr::from(([127, 0, 0, 1], port)),
            };
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut ready = false;
            while Instant::now() < deadline {
                if let Ok(Some(status)) = child.try_wait() {
                    last_err = format!("anvil exited early with {status}");
                    break;
                }
                if endpoint.try_rpc("eth_chainId", json!([])).is_ok() {
                    ready = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if ready {
                let mut anvil = Anvil {
                    child,
                    endpoint,
                    accounts: vec![],
                };
                anvil.accounts = anvil
                    .rpc("eth_accounts", json!([]))
                    .as_array()
                    .expect("eth_accounts array")
                    .iter()
                    .map(|a| a.as_str().unwrap().to_lowercase())
                    .collect();
                return anvil;
            }
            let _ = child.kill();
            let _ = child.wait();
            if last_err.is_empty() {
                last_err = "anvil did not answer eth_chainId within 20s".to_string();
            }
        }
        panic!("could not start anvil: {last_err}");
    }

    /// Cloneable endpoint handle (for the outcall router).
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    pub fn url(&self) -> String {
        self.endpoint.url()
    }

    /// JSON-RPC call returning `result`; panics with the error object on failure.
    pub fn rpc(&self, method: &str, params: Value) -> Value {
        self.endpoint
            .try_rpc(method, params)
            .unwrap_or_else(|e| panic!("anvil {method} failed: {e}"))
    }

    pub fn try_rpc(&self, method: &str, params: Value) -> Result<Value, Value> {
        self.endpoint.try_rpc(method, params)
    }

    pub fn chain_id(&self) -> u64 {
        parse_hex_u64(&self.rpc("eth_chainId", json!([])))
    }

    /// Mine `n` empty blocks (`anvil_mine`).
    pub fn mine(&self, n: u64) {
        self.rpc("anvil_mine", json!([format!("0x{n:x}")]));
    }

    /// `evm_snapshot`: returns an opaque snapshot id. Snapshots are consumed by
    /// [`Anvil::revert`] (and so are all snapshots taken after them).
    pub fn snapshot(&self) -> String {
        self.rpc("evm_snapshot", json!([]))
            .as_str()
            .expect("snapshot id string")
            .to_string()
    }

    /// `evm_revert` to a snapshot. Returns anvil's boolean success flag. Used by
    /// later scenario tests to simulate a reorg: blocks mined after the snapshot
    /// (including any bridgeOut they contained) disappear.
    pub fn revert(&self, id: &str) -> bool {
        self.rpc("evm_revert", json!([id]))
            .as_bool()
            .unwrap_or(false)
    }

    /// `anvil_setBalance`.
    pub fn set_balance(&self, addr: &str, wei: u128) {
        self.rpc("anvil_setBalance", json!([addr, format!("0x{wei:x}")]));
    }

    /// Block number for a tag (`latest`, `safe`, `finalized`, `earliest`,
    /// `pending`) or an explicit `0x..` number.
    pub fn block_number(&self, tag: &str) -> u64 {
        let blk = self.rpc("eth_getBlockByNumber", json!([tag, false]));
        parse_hex_u64(&blk["number"])
    }

    /// `eth_sendTransaction` from one of anvil's unlocked accounts. `to = None`
    /// deploys a contract. Returns only after the transaction is mined (anvil
    /// automines each transaction into its own block).
    ///
    /// Anvil does NOT reject a reverting call up front: it mines it and the
    /// receipt carries `status: 0x0`. Check [`Anvil::tx_succeeded`] when a revert
    /// is the expected outcome. Panics only if the node refuses the transaction
    /// outright (bad nonce, insufficient balance, unknown sender); use
    /// [`Anvil::try_send_tx_from_unlocked`] to observe that.
    pub fn send_tx_from_unlocked(
        &self,
        from: &str,
        to: Option<&str>,
        data: &[u8],
        value: u128,
    ) -> String {
        self.try_send_tx_from_unlocked(from, to, data, value, None)
            .unwrap_or_else(|e| panic!("eth_sendTransaction from {from} failed: {e}"))
    }

    /// Like [`Anvil::send_tx_from_unlocked`] but returns the node's error object
    /// as text and accepts an explicit gas limit.
    pub fn try_send_tx_from_unlocked(
        &self,
        from: &str,
        to: Option<&str>,
        data: &[u8],
        value: u128,
        gas: Option<u64>,
    ) -> Result<String, String> {
        let mut tx = json!({
            "from": from,
            "data": format!("0x{}", hex::encode(data)),
            "value": format!("0x{value:x}"),
        });
        if let Some(to) = to {
            tx["to"] = json!(to);
        }
        if let Some(gas) = gas {
            tx["gas"] = json!(format!("0x{gas:x}"));
        }
        let hash = self
            .try_rpc("eth_sendTransaction", json!([tx]))
            .map(|v| v.as_str().expect("tx hash string").to_string())
            .map_err(|e| e.to_string())?;
        // Anvil's automine runs on a separate task, so the receipt can lag the
        // RPC reply by a few milliseconds. Wait for it so callers can read the
        // receipt and block number immediately after sending.
        self.wait_for_receipt(&hash, Duration::from_secs(10))
            .ok_or_else(|| format!("tx {hash} was not mined within 10s"))?;
        Ok(hash)
    }

    /// Poll `eth_getTransactionReceipt` until the transaction is mined.
    pub fn wait_for_receipt(&self, hash: &str, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(r) = self.receipt(hash) {
                return Some(r);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// True when the mined transaction's receipt has `status == 0x1`.
    pub fn tx_succeeded(&self, hash: &str) -> bool {
        self.receipt(hash)
            .map(|r| r["status"] == "0x1")
            .unwrap_or(false)
    }

    /// `eth_getTransactionReceipt`; `None` while the transaction is unknown or
    /// not mined (for example after a snapshot revert).
    pub fn receipt(&self, hash: &str) -> Option<Value> {
        let r = self.rpc("eth_getTransactionReceipt", json!([hash]));
        if r.is_null() {
            None
        } else {
            Some(r)
        }
    }

    /// `eth_call` at `latest`. Returns the raw return bytes; panics on revert.
    pub fn eth_call(&self, to: &str, data: &[u8]) -> Vec<u8> {
        self.try_eth_call(to, data)
            .unwrap_or_else(|e| panic!("eth_call to {to} reverted: {e}"))
    }

    pub fn try_eth_call(&self, to: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        let r = self
            .try_rpc(
                "eth_call",
                json!([{"to": to, "data": format!("0x{}", hex::encode(data))}, "latest"]),
            )
            .map_err(|e| e.to_string())?;
        let s = r.as_str().ok_or("eth_call result not a string")?;
        hex::decode(s.trim_start_matches("0x")).map_err(|e| e.to_string())
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Parse a JSON `0x..` quantity into u64.
pub fn parse_hex_u64(v: &Value) -> u64 {
    let s = v
        .as_str()
        .unwrap_or_else(|| panic!("expected hex string, got {v}"));
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
        .unwrap_or_else(|e| panic!("bad hex quantity {s}: {e}"))
}
