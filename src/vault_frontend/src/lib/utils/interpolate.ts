/**
 * Piecewise linear interpolation over a sorted curve.
 * curve: array of [crLevel, multiplier] pairs, sorted ascending by crLevel.
 * Returns the interpolated multiplier for the given CR.
 */
export function interpolateMultiplier(curve: [number, number][], cr: number): number {
  if (curve.length === 0) return 1;
  if (cr <= curve[0][0]) return curve[0][1];
  if (cr >= curve[curve.length - 1][0]) return curve[curve.length - 1][1];
  for (let i = 0; i < curve.length - 1; i++) {
    if (cr >= curve[i][0] && cr <= curve[i + 1][0]) {
      const range = curve[i + 1][0] - curve[i][0];
      if (range === 0) return curve[i][1];
      const t = (cr - curve[i][0]) / range;
      return curve[i][1] + t * (curve[i + 1][1] - curve[i][1]);
    }
  }
  return 1;
}
