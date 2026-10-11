/// <reference types="node" />
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { resolve, join, basename } from "node:path";
import { existsSync, readFileSync } from "node:fs";

const host = process.env.TAURI_DEV_HOST;

/**
 * Debug builds of the app mirror their live state into the app data dir; serving it here
 * means `pnpm dev` in a plain browser shows the same real figures as the app window.
 *
 * The mirror is opt-in: the debug backend writes it only when it was started with
 * `PULSE_DEV_MIRROR=1` in its environment. Without that nothing is written, every route below
 * answers 404 with `{}`, and a plain browser shows the empty state ("No data yet") instead of
 * figures - which is the right answer for a backend that was not asked to share them. Set the
 * variable for the backend (`PULSE_DEV_MIRROR=1 pnpm tauri dev`) when the browser view is wanted.
 */
function pulseFixtures(): Plugin {
  const dir = join(process.env.APPDATA ?? "", "com.tzz.pulse", "dev");
  return {
    name: "pulse-fixtures",
    configureServer(server) {
      server.middlewares.use("/fixtures", (req, res) => {
        const name = basename(decodeURIComponent((req.url ?? "").replace(/^\/fixtures\/?/, "")));
        const file = join(dir, name);
        if (!name.endsWith(".json") || !existsSync(file)) {
          res.statusCode = 404;
          res.end("{}");
          return;
        }
        res.setHeader("content-type", "application/json");
        res.setHeader("cache-control", "no-store");
        res.end(readFileSync(file));
      });
    },
  };
}

export default defineConfig({
  base: "./",
  plugins: [tailwindcss(), react(), pulseFixtures()],
  resolve: {
    alias: { "@": resolve(__dirname, "src") },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
  },
  build: {
    target: ["chrome100", "safari15"],
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        panel: resolve(__dirname, "panel.html"),
      },
    },
  },
});
