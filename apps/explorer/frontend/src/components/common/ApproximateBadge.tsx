interface Props {
  sources: string[];
}

export function ApproximateBadge({ sources }: Props) {
  if (sources.length === 0) return null;
  const tooltip = `Historical points are approximated for: ${sources.join(", ")}. Rightmost point matches live values.`;
  return (
    <span
      className="inline-flex items-center gap-1 text-xs text-muted-foreground border border-border rounded-md px-1.5 py-0.5"
      title={tooltip}
    >
      ⓘ approximate
    </span>
  );
}
