/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      // Theme-driven colours (web/src/utils/theme.ts sets the variables).
      // `white` is the text/foreground colour and `neutral-*` is mixed from
      // text and background, so light themes invert without touching
      // components. Defaults in index.css keep the original Obsidian look.
      colors: {
        white: 'rgb(var(--c-fg) / <alpha-value>)',
        // Overlays and scrims: true black in dark themes, a soft ink in light.
        black: 'rgb(var(--c-shade) / <alpha-value>)',
        page: 'rgb(var(--c-bg) / <alpha-value>)',
        surface: 'rgb(var(--c-surface) / <alpha-value>)',
        shade: 'rgb(var(--c-shade) / <alpha-value>)',
        neutral: Object.fromEntries(
          [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950].map((n) => [n, `rgb(var(--c-n${n}) / <alpha-value>)`])
        ),
        obsidian: {
          950: '#000000',
          900: '#070709',
          850: '#0d0d11',
          800: '#141418',
          700: '#1c1c22',
          600: '#282830',
        },
        glass: {
          base: 'rgba(20, 20, 24, 0.75)',
          panel: 'rgba(26, 26, 31, 0.7)',
          card: 'rgba(34, 34, 40, 0.65)',
          bubble: 'rgba(38, 38, 46, 0.8)',
          userBubble: 'rgba(50, 50, 60, 0.85)',
          pill: 'rgba(255, 255, 255, 0.08)',
          pillHover: 'rgba(255, 255, 255, 0.14)',
          border: 'rgba(255, 255, 255, 0.09)',
          borderLight: 'rgba(255, 255, 255, 0.16)',
        },
        accent: {
          DEFAULT: 'rgb(var(--c-accent) / <alpha-value>)',
          fg: 'rgb(var(--c-accent-fg) / <alpha-value>)',
          blue: '#3b82f6',
          emerald: '#10b981',
          amber: '#f59e0b',
        }
      },
      // Hairlines follow the theme's border colour, separately from text.
      borderColor: {
        white: 'rgb(var(--c-border) / <alpha-value>)',
      },
      divideColor: {
        white: 'rgb(var(--c-border) / <alpha-value>)',
      },
      borderRadius: {
        'panel': '26px',
        'card-lg': '20px',
        'card-md': '14px',
        'pill': '9999px',
      },
      boxShadow: {
        'glass': '0 20px 50px rgba(0, 0, 0, 0.7), inset 0 1px 0 rgba(255, 255, 255, 0.12)',
        'dock': '0 10px 40px rgba(0, 0, 0, 0.8), inset 0 1px 1px rgba(255, 255, 255, 0.2)',
        'glow-white': '0 0 25px rgb(var(--c-accent) / 0.35)',
        'glow-emerald': '0 0 20px rgba(16, 185, 129, 0.3)',
        'glow-amber': '0 0 20px rgba(245, 158, 11, 0.35)',
      },
      backdropBlur: {
        'xs': '2px',
        '3xl': '28px',
      }
    },
  },
  plugins: [],
}
