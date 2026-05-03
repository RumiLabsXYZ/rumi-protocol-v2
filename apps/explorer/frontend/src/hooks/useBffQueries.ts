import { useQuery } from "@tanstack/react-query";
import { getBff } from "@/lib/bff";
import { toBffCursor, toBffFilter, type ActivityFilters } from "@/lib/activityFilters";
import { Principal } from "@icp-sdk/core/principal";

export function useOverview() {
  return useQuery({
    queryKey: ["overview"],
    queryFn: async () => {
      const bff = getBff();
      return bff.get_overview();
    },
    staleTime: 10_000,
  });
}

export function useHealth() {
  return useQuery({
    queryKey: ["health"],
    queryFn: async () => {
      const bff = getBff();
      return bff.get_health();
    },
    staleTime: 5_000,
    refetchInterval: 10_000,
  });
}

export function useActivity(filters: ActivityFilters) {
  return useQuery({
    queryKey: ["activity", JSON.stringify(filters, (_k, v) => typeof v === "bigint" ? v.toString() : v)],
    queryFn: async () => {
      const bff = getBff();
      return bff.get_activity(toBffFilter(filters), toBffCursor(filters));
    },
    staleTime: 5_000,
  });
}

export function useAddress(principalText: string) {
  return useQuery({
    queryKey: ["address", principalText],
    queryFn: async () => {
      const bff = getBff();
      let p: Principal;
      try {
        p = Principal.fromText(principalText);
      } catch {
        throw new Error(`Invalid principal: ${principalText}`);
      }
      return bff.get_address(p);
    },
    staleTime: 30_000,
    retry: false,
  });
}

export function useVault(idStr: string) {
  return useQuery({
    queryKey: ["vault", idStr],
    queryFn: async () => {
      const bff = getBff();
      const id = BigInt(idStr);
      return bff.get_vault(id);
    },
    staleTime: 30_000,
    retry: false,
  });
}

export function usePool(poolId: string) {
  return useQuery({
    queryKey: ["pool", poolId],
    queryFn: async () => getBff().get_pool(poolId),
    staleTime: 30_000,
  });
}

export function useToken(ledgerText: string) {
  return useQuery({
    queryKey: ["token", ledgerText],
    queryFn: async () => {
      const bff = getBff();
      let p: Principal;
      try {
        p = Principal.fromText(ledgerText);
      } catch {
        throw new Error(`Invalid ledger: ${ledgerText}`);
      }
      return bff.get_token(p);
    },
    staleTime: 60_000,
    retry: false,
  });
}

export function useEvent(globalId: string) {
  return useQuery({
    queryKey: ["event", globalId],
    queryFn: async () => getBff().get_event(globalId),
    staleTime: Infinity,
  });
}

export function useLensCollateral() {
  return useQuery({
    queryKey: ["lens", "collateral"],
    queryFn: () => getBff().get_lens_collateral(),
    staleTime: 60_000,
  });
}

export function useLensStabilityPool() {
  return useQuery({
    queryKey: ["lens", "stability"],
    queryFn: () => getBff().get_lens_stability_pool(),
    staleTime: 60_000,
  });
}

export function useLensRevenue() {
  return useQuery({
    queryKey: ["lens", "revenue"],
    queryFn: () => getBff().get_lens_revenue(),
    staleTime: 60_000,
  });
}

export function useLensRedemptions() {
  return useQuery({
    queryKey: ["lens", "redemptions"],
    queryFn: () => getBff().get_lens_redemptions(),
    staleTime: 60_000,
  });
}

export function useLensDex() {
  return useQuery({
    queryKey: ["lens", "dex"],
    queryFn: () => getBff().get_lens_dex(),
    staleTime: 60_000,
  });
}

export function useLensAdmin() {
  return useQuery({
    queryKey: ["lens", "admin"],
    queryFn: () => getBff().get_lens_admin(),
    staleTime: 60_000,
  });
}
