use crate::read_state;
use std::io::Write;

pub fn build_dashboard() -> Vec<u8> {
    format!(
        "
    <!DOCTYPE html>
    <html lang=\"en\">
        <head>
            <title>Elliptic Protocol Dashboard</title>
            <style>
                table {{
                    border: solid;
                    text-align: left;
                    width: 100%;
                    border-width: thin;
                }}
                h3 {{
                    font-variant: small-caps;
                    margin-top: 30px;
                    margin-bottom: 5px;
                }}
                table table {{ font-size: small; }}
                .background {{ margin: 0; padding: 0; }}
                .content {{ max-width: 100vw; width: fit-content; margin: 0 auto; }}
                tbody tr:nth-child(odd) {{ background-color: #eeeeee; }}
            </style>
            <script>
                document.addEventListener(\"DOMContentLoaded\", function() {{
                    var tds = document.querySelectorAll(\".ts-class\");
                    for (var i = 0; i < tds.length; i++) {{
                    var td = tds[i];
                    var timestamp = td.textContent / 1000000;
                    var date = new Date(timestamp);
                    var options = {{
                        year: 'numeric',
                        month: 'short',
                        day: 'numeric',
                        hour: 'numeric',
                        minute: 'numeric',
                        second: 'numeric'
                    }};
                    td.title = td.textContent;
                    td.textContent = date.toLocaleString(undefined, options);
                    }}
                }});
            </script>
        </head>
        <body>
            <div class=\"background content\">
                <div>
                    <h3>Metadata</h3>
                    {}
                </div>
                <div>
                    <h3>Collateral Types</h3>
                    {}
                </div>
                <div>
                    <h3>Vault Table</h3>
                    <table>
                        <thead>
                            <tr>
                                <th>Vault Id</th>
                                <th>Owner</th>
                                <th>Borrowed icUSD</th>
                                <th>Collateral Amount</th>
                                <th>Collateral Type</th>
                            </tr>
                        </thead>
                        <tbody>{}</tbody>
                    </table>
                </div>
                <div>
                    <h3>Liquidity Table</h3>
                    <table>
                        <thead>
                            <tr>
                                <th>Owner</th>
                                <th>Amount</th>
                            </tr>
                        </thead>
                        <tbody>{}</tbody>
                    </table>
                </div>
                <div>
                    <h3>Liquidity Rewards</h3>
                    <table>
                        <thead>
                            <tr>
                                <th>Owner</th>
                                <th>Amount</th>
                            </tr>
                        </thead>
                        <tbody>{}</tbody>
                    </table>
                </div>
                <h3>Logs</h3>
                <table>
                    <thead>
                        <tr><th>Priority</th><th>Timestamp</th><th>Location</th><th>Message</th></tr>
                    </thead>
                    <tbody>
                        {}
                    </tbody>
                </table>
            </div>
        </body>
    </html>
    ",
        construct_metadata_table(),
        construct_collateral_types_table(),
        construct_vault_table(),
        construct_liquidity_table(),
        construct_liquidity_returns(),
        display_logs()
    )
    .into_bytes()
}

fn with_utf8_buffer(f: impl FnOnce(&mut Vec<u8>)) -> String {
    let mut buf = Vec::new();
    f(&mut buf);
    String::from_utf8(buf).unwrap()
}

fn construct_metadata_table() -> String {
    read_state(|s| {
        let last_icp_rate = s.last_icp_rate;
        let last_icp_timetsamp = s.last_icp_timestamp;
        format!(
            "<table>
                <tbody>
                    <tr>
                        <th>Mode</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>icUSD Ledger Principal</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>ICP Ledger Principal</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>XRC Principal</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>BTC Rate</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>Last BTC Price Timestamp</th>
                        <td class=\"ts-class\">{}</td>
                    </tr>
                    <tr>
                        <th>Total Collateral Ratio</th>
                        <td>{}%</td>
                    </tr>
                    <tr>
                        <th>Total circulating TAL</th>
                        <td>{}</td>
                    </tr>
                    <tr>
                        <th>Borrowing Fee</th>
                        <td>{}%</td>
                    </tr>
                    <tr>
                    <th>Last Redemption Fee</th>
                        <td>{}%</td>
                    </tr>
                    <tr>
                        <th>Vault Id</th>
                        <td>{}</td>
                    </tr>
                </tbody>
            </table>",
            s.mode,
            s.icusd_ledger_principal,
            s.icp_ledger_principal,
            s.xrc_principal,
            last_icp_rate.unwrap_or(crate::UsdIcp::from(rust_decimal::Decimal::ZERO)),
            last_icp_timetsamp.unwrap_or(0),
            s.total_collateral_ratio.to_f64() * 100.0,
            s.total_borrowed_icusd_amount(),
            s.fee.to_f64() * 100.0,
            s.current_base_rate.to_f64() * 100.0,
            s.next_available_vault_id
        )
    })
}

fn construct_vault_table() -> String {
    with_utf8_buffer(|buf| {
        read_state(|s| {
            for (_vault_id, vault) in s.vault_id_to_vaults.iter() {
                // Shorten collateral type to first 8 chars for display
                let ct_short = vault.collateral_type.to_string();
                let ct_display = if ct_short.len() > 12 {
                    format!("{}...", &ct_short[..12])
                } else {
                    ct_short
                };
                write!(
                    buf,
                    "
                <tr>
                    <td>{}</td>
                    <td>{}</td>
                    <td>{}</td>
                    <td>{}</td>
                    <td title=\"{}\">{}</td>
                </tr>
                ",
                    vault.vault_id,
                    vault.owner,
                    vault.borrowed_icusd_amount,
                    vault.collateral_amount,
                    vault.collateral_type,
                    ct_display,
                )
                .unwrap();
            }
            write!(
                buf,
                "<tr><td colspan='2' style='text-align: right;'><b>Total</b></td><td>{}</td><td>{}</td><td></td></tr>",
                s.total_borrowed_icusd_amount(),
                s.total_icp_margin_amount()
            )
            .unwrap();
        });
    })
}

fn construct_collateral_types_table() -> String {
    with_utf8_buffer(|buf| {
        read_state(|s| {
            write!(
                buf,
                "<table>
                    <thead>
                        <tr>
                            <th>Ledger</th>
                            <th>Status</th>
                            <th>Decimals</th>
                            <th>Last Price (USD)</th>
                            <th>Total Collateral</th>
                            <th>Total Debt</th>
                            <th>Vault Count</th>
                            <th>Debt Ceiling</th>
                        </tr>
                    </thead>
                    <tbody>"
            )
            .unwrap();

            for (ct, config) in &s.collateral_configs {
                let total_raw = s.total_collateral_for(ct);
                let total_debt = s.total_debt_for_collateral(ct);
                let vault_count = s.collateral_to_vault_ids.get(ct).map_or(0, |ids| ids.len());
                let price_str = config.last_price.map_or("N/A".to_string(), |p| format!("{:.4}", p));
                let ceiling_str = if config.debt_ceiling == u64::MAX {
                    "unlimited".to_string()
                } else {
                    format!("{}", config.debt_ceiling)
                };
                write!(
                    buf,
                    "<tr>
                        <td title=\"{}\">{}</td>
                        <td>{:?}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                        <td>{}</td>
                    </tr>",
                    ct,
                    &ct.to_string()[..std::cmp::min(ct.to_string().len(), 12)],
                    config.status,
                    config.decimals,
                    price_str,
                    total_raw,
                    total_debt,
                    vault_count,
                    ceiling_str,
                )
                .unwrap();
            }

            write!(buf, "</tbody></table>").unwrap();
        });
    })
}

fn construct_liquidity_table() -> String {
    with_utf8_buffer(|buf| {
        read_state(|s| {
            for (principal, amount) in s.liquidity_pool.iter() {
                write!(
                    buf,
                    "
                <tr>
                    <td>{}</td>
                    <td>{}</td>
                </tr>
                ",
                    principal,
                    (*amount)
                )
                .unwrap();
            }
            write!(
                buf,
                "<tr><td colspan='1' style='text-align: right;'><b>Total Liquidity Provided</b></td><td>{}</td></tr>",
                s.total_provided_liquidity_amount()
            )
            .unwrap();
        });
    })
}

fn construct_liquidity_returns() -> String {
    with_utf8_buffer(|buf| {
        read_state(|s| {
            for (principal, amount) in s.liquidity_returns.iter() {
                write!(buf, "<tr><td>{}</td><td>{}</td></tr>", principal, (*amount)).unwrap();
            }
            write!(
                buf,
                "<tr><td colspan='1' style='text-align: right;'><b>Total Rewards Available</b></td><td>{}</td></tr>",
                s.total_available_returns()
            )
            .unwrap();
        })
    })
}

fn display_logs() -> String {
    use crate::logs::{Log, LogEntry};

    fn display_entry(buf: &mut Vec<u8>, e: &LogEntry) {
        write!(
            buf,
            "<tr><td>{:?}</td><td class=\"ts-class\">{}</td><td><code>{}:{}</code></td><td>{}</td></tr>",
            e.priority, e.timestamp, e.file, e.line, e.message
        )
        .unwrap()
    }

    let mut log: Log = Default::default();
    log.push_all();
    log.entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    with_utf8_buffer(|buf| {
        for e in log.entries {
            display_entry(buf, &e);
        }
    })
}
