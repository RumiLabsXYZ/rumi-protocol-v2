import type { ActivityFilter, ActivityCursor } from "@/bindings/explorer_bff/explorer_bff";

export interface ActivityFilters {
  types: string[];
  fromTs: bigint | null;
  toTs: bigint | null;
  cursor: string | null;
  pageSize: number;
}

const DEFAULT_PAGE_SIZE = 25;

export function parseFilters(params: URLSearchParams): ActivityFilters {
  const types = params.get("type")?.split(",").filter(Boolean) ?? [];
  const fromStr = params.get("from");
  const toStr = params.get("to");
  const cursor = params.get("before");
  const pageSizeStr = params.get("size");

  const fromTs = fromStr ? dateToNs(fromStr) : null;
  const toTs = toStr ? dateToNs(toStr) : null;
  const pageSize = pageSizeStr ? Number(pageSizeStr) : DEFAULT_PAGE_SIZE;

  return { types, fromTs, toTs, cursor, pageSize };
}

export function filtersToParams(f: ActivityFilters): URLSearchParams {
  const params = new URLSearchParams();
  if (f.types.length > 0) params.set("type", f.types.join(","));
  if (f.fromTs) params.set("from", nsToDate(f.fromTs));
  if (f.toTs) params.set("to", nsToDate(f.toTs));
  if (f.cursor) params.set("before", f.cursor);
  if (f.pageSize !== DEFAULT_PAGE_SIZE) params.set("size", String(f.pageSize));
  return params;
}

// Convert filter object to the candid ActivityFilter shape.
// The bindgen actor uses idiomatic optional fields (T | undefined), not tuple opt encoding.
// The field name is `filter_principal` (not `principal`) due to a Candid keyword collision.
export function toBffFilter(f: ActivityFilters): ActivityFilter {
  return {
    sources: undefined,
    types: f.types.length > 0 ? f.types : undefined,
    filter_principal: undefined,
    from_ns: f.fromTs !== null ? f.fromTs : undefined,
    to_ns: f.toTs !== null ? f.toTs : undefined,
  };
}

export function toBffCursor(f: ActivityFilters): ActivityCursor {
  return {
    before_global_id: f.cursor ?? undefined,
    page_size: f.pageSize,
  };
}

function dateToNs(date: string): bigint {
  const ms = Date.parse(date + "T00:00:00Z");
  if (isNaN(ms)) return 0n;
  return BigInt(ms) * 1_000_000n;
}

function nsToDate(ns: bigint): string {
  const ms = Number(ns / 1_000_000n);
  const d = new Date(ms);
  return d.toISOString().split("T")[0];
}
