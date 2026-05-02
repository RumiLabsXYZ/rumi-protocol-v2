import { Outlet } from "react-router-dom";
import { TopNav } from "./TopNav";
import { Footer } from "./Footer";

export function Layout() {
  return (
    <div className="min-h-screen flex flex-col bg-background text-foreground">
      <TopNav />
      <main className="container mx-auto flex-1 py-6">
        <Outlet />
      </main>
      <Footer />
    </div>
  );
}
