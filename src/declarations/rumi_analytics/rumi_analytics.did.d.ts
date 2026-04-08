import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface DailyTvlRow {
  'timestamp_ns' : bigint,
  'total_icusd_supply_e8s' : bigint,
  'system_collateral_ratio_bps' : number,
  'total_icp_collateral_e8s' : bigint,
}
export type HeaderField = [string, string];
export interface HttpRequest {
  'url' : string,
  'method' : string,
  'body' : Uint8Array | number[],
  'headers' : Array<HeaderField>,
}
export interface HttpResponse {
  'body' : Uint8Array | number[],
  'headers' : Array<HeaderField>,
  'status_code' : number,
}
export interface InitArgs {
  'amm' : Principal,
  'three_pool' : Principal,
  'admin' : Principal,
  'icusd_ledger' : Principal,
  'stability_pool' : Principal,
  'backend' : Principal,
}
export interface RangeQuery {
  'to_ts' : [] | [bigint],
  'from_ts' : [] | [bigint],
  'offset' : [] | [bigint],
  'limit' : [] | [number],
}
export interface TvlSeriesResponse {
  'rows' : Array<DailyTvlRow>,
  'next_from_ts' : [] | [bigint],
}
export interface _SERVICE {
  'get_admin' : ActorMethod<[], Principal>,
  'get_tvl_series' : ActorMethod<[RangeQuery], TvlSeriesResponse>,
  'http_request' : ActorMethod<[HttpRequest], HttpResponse>,
  'ping' : ActorMethod<[], string>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
