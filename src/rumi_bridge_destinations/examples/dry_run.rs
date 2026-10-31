//! Dry run: load the embedded profiles, validate them for the date given as
//! the first argument, and print one line per network. Makes no network calls.
//!
//! Usage: cargo run --offline -p rumi_bridge_destinations --example dry_run -- 2026-10-30

use std::process::exit;

use rumi_bridge_destinations::{label, load_embedded, validate};

fn main() {
    let Some(today) = std::env::args().nth(1) else {
        eprintln!("usage: dry_run YYYY-MM-DD");
        exit(2);
    };
    let profiles = match load_embedded() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not load embedded profiles: {e}");
            exit(1);
        }
    };
    let mut networks = 0;
    for p in &profiles {
        for n in &p.networks {
            networks += 1;
            let blockers: Vec<&str> = n
                .activation_blockers
                .iter()
                .map(|b| b.id.as_str())
                .collect();
            println!(
                "{} | family={} | status={} | blockers={}",
                n.network_id,
                label(&p.family),
                label(&n.status),
                blockers.join(",")
            );
        }
    }
    match validate(&profiles, &today) {
        Ok(()) => println!(
            "OK: {} profiles, {networks} networks, 0 violations (date {today})",
            profiles.len()
        ),
        Err(violations) => {
            for v in &violations {
                eprintln!("{v}");
            }
            eprintln!("FAILED: {} violation(s)", violations.len());
            exit(1);
        }
    }
}
