import type { CSSProperties } from "react";

interface Props {
  /** Collateral ratio as a multiple (e.g., 1.62 means 162% CR). */
  ratio?: number | null;
  /** Status visual variant. */
  status?: "open" | "closed" | "liquidated";
  /** Pixel size — defaults to 16. */
  size?: number;
  /** Inline title for accessibility. */
  title?: string;
}

const RATIO_FULL = 2.0; // 200% CR = full glyph
const RATIO_DANGER = 1.1; // below this, fill renders cinnabar

/**
 * VaultGlyph — a small stratified rectangle. Fill height = collateral ratio,
 * capped at 200%. Closed vaults appear as an empty outline. Liquidated vaults
 * appear with a cinnabar diagonal.
 *
 * Used inline in tables, vault detail headers, and the overview's vault count
 * card. The signature element of the Rumi explorer alongside the peg meridian.
 */
export function VaultGlyph({
  ratio,
  status = "open",
  size = 16,
  title,
}: Props) {
  const width = size * 0.72;
  const height = size;
  const strokeColor = "hsl(var(--ink-secondary))";
  const fillColor =
    typeof ratio === "number" && ratio < RATIO_DANGER
      ? "hsl(var(--cinnabar) / 0.65)"
      : "hsl(var(--verdigris) / 0.55)";

  const fillRatio =
    typeof ratio === "number"
      ? Math.max(0, Math.min(ratio / RATIO_FULL, 1))
      : 0;
  const fillHeight = (height - 2) * fillRatio;
  const fillY = 1 + (height - 2 - fillHeight);

  const style: CSSProperties = {
    display: "inline-block",
    verticalAlign: "middle",
  };

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      style={style}
      role="img"
      aria-label={title ?? `Vault (CR ${ratio?.toFixed(2) ?? "—"})`}
    >
      {title && <title>{title}</title>}
      {/* Outline */}
      <rect
        x="0.5"
        y="0.5"
        width={width - 1}
        height={height - 1}
        fill="none"
        stroke={strokeColor}
        strokeWidth="1"
        rx="1"
      />
      {/* Fill (only on Open vaults) */}
      {status === "open" && fillHeight > 0 && (
        <rect
          x="1.5"
          y={fillY}
          width={width - 3}
          height={fillHeight}
          fill={fillColor}
        />
      )}
      {/* Liquidated diagonal */}
      {status === "liquidated" && (
        <line
          x1="1"
          y1={height - 1}
          x2={width - 1}
          y2="1"
          stroke="hsl(var(--cinnabar))"
          strokeWidth="1"
        />
      )}
    </svg>
  );
}
