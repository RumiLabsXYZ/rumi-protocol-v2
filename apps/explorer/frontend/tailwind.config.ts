import type { Config } from "tailwindcss";

const config: Config = {
  darkMode: ["class"],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    container: {
      center: true,
      padding: "1.5rem",
      screens: { "2xl": "1400px" },
    },
    extend: {
      colors: {
        // ── Existing shadcn-named tokens (back-compat; resolve through CSS vars) ──
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        warning: {
          DEFAULT: "hsl(var(--warning))",
          foreground: "hsl(var(--warning-foreground))",
        },
        success: {
          DEFAULT: "hsl(var(--success))",
          foreground: "hsl(var(--success-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },

        // ── New domain-specific tokens (use these for new code) ──
        vellum: {
          DEFAULT: "hsl(var(--vellum))",
          raised: "hsl(var(--vellum-raised))",
          inset: "hsl(var(--vellum-inset))",
        },
        ink: {
          DEFAULT: "hsl(var(--ink-primary))",
          primary: "hsl(var(--ink-primary))",
          secondary: "hsl(var(--ink-secondary))",
          muted: "hsl(var(--ink-muted))",
          disabled: "hsl(var(--ink-disabled))",
        },
        quartz: {
          DEFAULT: "hsl(var(--quartz-rule))",
          rule: "hsl(var(--quartz-rule))",
          soft: "hsl(var(--quartz-rule-soft))",
          emphasis: "hsl(var(--quartz-rule-emphasis))",
        },
        verdigris: {
          DEFAULT: "hsl(var(--verdigris))",
          soft: "hsl(var(--verdigris-soft))",
          foreground: "hsl(var(--verdigris-fg))",
        },
        sodium: {
          DEFAULT: "hsl(var(--sodium))",
          soft: "hsl(var(--sodium-soft))",
          foreground: "hsl(var(--sodium-fg))",
        },
        cinnabar: {
          DEFAULT: "hsl(var(--cinnabar))",
          soft: "hsl(var(--cinnabar-soft))",
          foreground: "hsl(var(--cinnabar-fg))",
        },
        peg: {
          DEFAULT: "hsl(var(--peg))",
          meridian: "hsl(var(--peg-meridian))",
        },
      },
      borderRadius: {
        sm: "var(--radius-sm)",
        md: "var(--radius)",
        lg: "var(--radius-lg)",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "ui-monospace", "monospace"],
      },
      letterSpacing: {
        tightest: "-0.02em",
      },
    },
  },
  plugins: [],
};

export default config;
