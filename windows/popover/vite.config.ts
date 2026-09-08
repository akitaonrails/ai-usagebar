import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import Icons from "unplugin-icons/vite";
import { FileSystemIconLoader } from "unplugin-icons/loaders";

const root = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  base: "./",
  plugins: [
    react(),
    tailwindcss(),
    Icons({
      compiler: "jsx",
      jsx: "react",
      customCollections: {
        aiub: FileSystemIconLoader(path.join(root, "src/icons/providers"), (svg) => svg),
      },
    }),
  ],
  resolve: {
    alias: {
      "@": path.join(root, "src"),
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    cssCodeSplit: false,
    assetsInlineLimit: 4096,
    rollupOptions: {
      output: {
        codeSplitting: false,
        entryFileNames: "popover.js",
        chunkFileNames: "popover.js",
        assetFileNames: (info) => {
          const name = info.names?.[0] || info.name || "";
          if (name.endsWith(".css")) return "popover.css";
          return "assets/[name][extname]";
        },
      },
    },
  },
});
