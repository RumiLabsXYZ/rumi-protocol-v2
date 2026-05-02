import { QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Route, Routes } from "react-router-dom";
import { queryClient } from "./lib/queryClient";
import { ThemeProvider } from "./theme/ThemeProvider";
import { Layout } from "./components/layout/Layout";
import { Overview } from "./pages/Overview";
import { Activity } from "./pages/Activity";
import { Health } from "./pages/Health";
import { CollateralLens } from "./pages/lenses/CollateralLens";
import { StabilityPoolLens } from "./pages/lenses/StabilityPoolLens";
import { RevenueLens } from "./pages/lenses/RevenueLens";
import { RedemptionsLens } from "./pages/lenses/RedemptionsLens";
import { DexLens } from "./pages/lenses/DexLens";
import { AdminLens } from "./pages/lenses/AdminLens";
import { AddressDetail } from "./pages/entity/AddressDetail";
import { VaultDetail } from "./pages/entity/VaultDetail";
import { PoolDetail } from "./pages/entity/PoolDetail";
import { TokenDetail } from "./pages/entity/TokenDetail";
import { EventDetail } from "./pages/entity/EventDetail";

export default function App() {
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>
          <Routes>
            <Route element={<Layout />}>
              <Route index element={<Overview />} />
              <Route path="activity" element={<Activity />} />
              <Route path="health" element={<Health />} />
              <Route path="lens/collateral" element={<CollateralLens />} />
              <Route path="lens/stability-pool" element={<StabilityPoolLens />} />
              <Route path="lens/revenue" element={<RevenueLens />} />
              <Route path="lens/redemptions" element={<RedemptionsLens />} />
              <Route path="lens/dex" element={<DexLens />} />
              <Route path="lens/admin" element={<AdminLens />} />
              <Route path="e/address/:principal" element={<AddressDetail />} />
              <Route path="e/vault/:id" element={<VaultDetail />} />
              <Route path="e/pool/:id" element={<PoolDetail />} />
              <Route path="e/token/:ledger" element={<TokenDetail />} />
              <Route path="e/event/:globalId" element={<EventDetail />} />
            </Route>
          </Routes>
        </BrowserRouter>
      </QueryClientProvider>
    </ThemeProvider>
  );
}
