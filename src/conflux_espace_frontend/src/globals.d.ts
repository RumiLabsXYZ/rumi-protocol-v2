/** Replaced by Vite before bundling. Mainnet builds set this to literal false,
 * allowing Rollup to remove the complete dev-wallet UI and implementation. */
declare const __ENABLE_DEV_KEY__: boolean;

/** Literal deployment-mode pins used inside Svelte component compilation so
 * non-target UI branches are dead code in production artifacts. */
declare const __RUMI_PRODUCTION_CANARY_BUILD__: boolean;
declare const __RUMI_PRODUCTION_PUBLIC_BUILD__: boolean;
