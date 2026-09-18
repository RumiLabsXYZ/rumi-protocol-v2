// Same theme tokens as src/vault_frontend/tailwind.config.js, kept in sync so
// this standalone frontend shares the main app's brand palette and motion.
export default {
  content: ["./index.html", "./src/**/*.{html,js,svelte,ts}"],
  theme: {
    extend: {
      colors: {
        primary: "#00b4d8",
        "gradient-start": "#8b5cf6",
        "gradient-end": "#d8b4fe",
      },
      animation: {
        "gradient-xy": "gradient-xy 15s ease infinite",
        "gradient-move": "gradientMove 15s ease infinite",
        "spin-slow": "spin 10s linear infinite",
        "spin-slow-reverse": "spin 10s linear infinite reverse",
      },
      keyframes: {
        "gradient-xy": {
          "0%, 100%": { transform: "translate(0, 0)" },
          "50%": { transform: "translate(-30%, -30%)" },
        },
        gradientMove: {
          "0%, 100%": { backgroundPosition: "0% 50%" },
          "50%": { backgroundPosition: "100% 50%" },
        },
      },
      backgroundImage: {
        "gradient-primary": "linear-gradient(135deg, var(--gradient-start) 0%, var(--gradient-end) 100%)",
      },
    },
  },
  plugins: [],
};
