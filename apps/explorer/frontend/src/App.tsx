import { QueryClientProvider } from "@tanstack/react-query";
import { queryClient } from "./lib/queryClient";
import { ThemeProvider } from "./theme/ThemeProvider";
import { ThemeToggle } from "./theme/ThemeToggle";

export default function App() {
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <main className="min-h-screen flex flex-col bg-background text-foreground">
          <header className="flex justify-between items-center px-6 py-4 border-b border-border">
            <h1 className="text-xl font-semibold">Rumi Explorer</h1>
            <ThemeToggle />
          </header>
          <section className="flex-1 flex items-center justify-center">
            <p className="text-muted-foreground">Theme + provider scaffolding ready.</p>
          </section>
        </main>
      </QueryClientProvider>
    </ThemeProvider>
  );
}
