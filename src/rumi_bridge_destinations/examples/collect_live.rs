//! Run the read-only collector with the LIVE client transport.
//!
//! Usage:
//!   cargo run -p rumi_bridge_destinations --features live-transport --example collect_live -- \
//!       <endpoints.json> <collect.json> --live
//!
//! * `<endpoints.json>` is the explicit endpoint config (`LiveConfig`): the IC
//!   replica URL, the ledger and backend canister ids, and per EVM network the
//!   token address and provider URLs. There are no built-in endpoints or ids.
//! * `<collect.json>` is a `CollectConfig`: the destinations (with their escrow
//!   bindings) and the optional XRPL lane to collect.
//! * `--live` is required. Running this example sends real queries to the
//!   endpoints in the config, so it refuses to start without it.
//!
//! The report starts with `source: live` and carries the read time and each EVM
//! head height. Exit codes: 0 report with no failure flag; 1 input rejected or a
//! reading missing or unavailable; 2 usage or an unreadable file; 3 a report
//! with a failure flag (credits must halt).

use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

use rumi_bridge_destinations::aggregate::Catalog;
use rumi_bridge_destinations::collect::{collect, render, CollectConfig, Readings};
use rumi_bridge_destinations::live::{LiveConfig, LiveReadings};

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> T {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            exit(2);
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{path}: {e}");
            exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(endpoints), Some(collect_path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: collect_live <endpoints.json> <collect.json> --live");
        exit(2);
    };
    if args.get(2).map(String::as_str) != Some("--live") || args.len() != 3 {
        eprintln!("refusing to run: this sends real queries to the endpoints in {endpoints}; pass --live to confirm");
        exit(2);
    }

    let live_config: LiveConfig = read_json(endpoints);
    let collect_config: CollectConfig = read_json(collect_path);
    let catalog = match Catalog::embedded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not load the embedded profiles: {e}");
            exit(2);
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let readings = match LiveReadings::from_config(&live_config, now) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("endpoint config: {e}");
            exit(2);
        }
    };

    match collect(&collect_config, &catalog, &readings) {
        Ok(collected) => {
            print!("{}", render(&collected));
            if collected.report.has_failure() {
                exit(3);
            }
        }
        Err(e) => {
            println!("source: {}", readings.source());
            println!("status: REJECTED, no report");
            eprintln!("{e}");
            exit(1);
        }
    }
}
