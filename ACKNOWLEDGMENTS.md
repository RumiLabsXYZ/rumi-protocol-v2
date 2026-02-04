# Acknowledgments

## Architectural Inspiration

Rumi Protocol's CDP (Collateralized Debt Position) stablecoin architecture is inspired by the [Liquity Protocol](https://www.liquity.org/) design, which pioneered decentralized, governance-free stablecoins with innovative stability mechanisms.

- Liquity Protocol: https://www.liquity.org/
- Liquity Documentation: https://docs.liquity.org/
- Liquity GitHub: https://github.com/liquity/dev

## Contributors

The following individuals have contributed to the development of Rumi Protocol:

### Agnes Koinange
- Treasury system implementation
- Stability Pool canister and frontend
- Partial vault operations (partial liquidation, partial repayment)
- Internet Identity wallet integration

### Rob Ripley
- Mainnet deployment and operations
- Wallet integration (Plug, OISY, Internet Identity)
- Custom domain configuration
- Protocol parameter tuning
- icUSD index canister integration

### Gurleen Dhaliwal
- Initial project setup and configuration
- Testing infrastructure (PocketIC integration)
- Frontend UI development
- Documentation

### Zaria Hallager
- ckBTC collateral logic

## Internet Computer Ecosystem

Rumi Protocol is built on the [Internet Computer](https://internetcomputer.org/) blockchain and leverages several ecosystem technologies:

- **XRC (Exchange Rate Canister)** - For decentralized price feeds
- **ckBTC / ckETH** - Chain-key wrapped assets for multi-collateral support
- **Internet Identity** - Decentralized authentication
- **ICRC-1/ICRC-2** - Token standards for icUSD

## Open Source Dependencies

This project uses numerous open source libraries and tools. See `package.json` and `Cargo.toml` for a complete list of dependencies.
