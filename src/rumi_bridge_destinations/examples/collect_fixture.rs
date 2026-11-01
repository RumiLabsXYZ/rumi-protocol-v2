//! Run the read-only collector over a fixture file and print the report.
//!
//! Usage: cargo run --offline -p rumi_bridge_destinations --example collect_fixture -- <fixture.json>
//!
//! The readings come from the local file; nothing is queried, so the output
//! always starts with `source: fixture` and can never read as live data.
//!
//! Exit codes: 0 report printed with no failure flag; 1 the input was
//! rejected or a reading is missing or unavailable; 2 usage or an unreadable
//! fixture; 3 a report was printed but carries a failure flag (credits must
//! halt).

use std::process::exit;

use rumi_bridge_destinations::aggregate::Catalog;
use rumi_bridge_destinations::collect::{collect, render, FixtureReadings, Readings};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: collect_fixture <fixture.json>");
        exit(2);
    };
    let json = match std::fs::read_to_string(&path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            exit(2);
        }
    };
    let fixture = match FixtureReadings::parse(&json) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path}: {e}");
            exit(2);
        }
    };
    let catalog = match Catalog::embedded() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not load the embedded profiles: {e}");
            exit(2);
        }
    };

    match collect(&fixture.config, &catalog, &fixture.readings) {
        // `render` starts with the `source:` line.
        Ok(collected) => {
            print!("{}", render(&collected));
            if collected.report.has_failure() {
                exit(3);
            }
        }
        Err(e) => {
            // Label first, so even a rejected run says where its numbers came from.
            println!("source: {}", fixture.readings.source());
            println!("fixture: {path}");
            println!("status: REJECTED, no report");
            eprintln!("{e}");
            exit(1);
        }
    }
}
