import { Component, type ReactNode } from "react";

interface State {
  err: Error | null;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { err: null };

  static getDerivedStateFromError(err: Error): State {
    return { err };
  }

  componentDidCatch(err: Error, info: unknown) {
    console.error("[ErrorBoundary]", err, info);
  }

  reset = () => this.setState({ err: null });

  render() {
    if (this.state.err) {
      return (
        <div className="container mx-auto py-12">
          <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-6">
            <p className="font-semibold mb-2">Something went wrong on this page.</p>
            <p className="text-sm mb-4">{this.state.err.message}</p>
            <button
              onClick={this.reset}
              className="text-xs px-3 py-1 rounded-md border border-destructive/40 hover:bg-destructive/20"
            >
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
