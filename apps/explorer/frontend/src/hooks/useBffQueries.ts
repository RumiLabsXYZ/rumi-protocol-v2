import { useQuery } from "@tanstack/react-query";
import { getBff } from "@/lib/bff";

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
