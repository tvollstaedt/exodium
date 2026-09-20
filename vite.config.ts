import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { createReadStream, existsSync, statSync } from "node:fs";
import { join, resolve, sep } from "node:path";

/** pdf.js loads its JPX/JBIG2 decoders by name from `public/pdfjs/` and, when
 *  WebAssembly is unavailable, `import()`s the JS fallback next to them. The
 *  dev server refuses to serve a /public script through its module pipeline
 *  ("should not be imported from source code"), which turned that fallback
 *  into a red overlay. Serving the directory raw, ahead of the transform
 *  middleware, gives dev the same behaviour a build has. */
const rawPdfjsRuntime = (): Plugin => ({
  name: "exodium:raw-pdfjs-runtime",
  configureServer(server) {
    const root = join(process.cwd(), "public", "pdfjs");
    server.middlewares.use((req, res, next) => {
      const url = (req.url ?? "").split("?")[0];
      if (!url.startsWith("/pdfjs/")) { return next(); }
      // The request path is whatever the client sends, and with TAURI_DEV_HOST
      // the dev server listens on the network - so it is resolved and has to
      // land inside the staged directory, `..` and %2e included.
      let file: string;
      try {
        file = resolve(root, decodeURIComponent(url.slice("/pdfjs/".length)));
      } catch {
        res.statusCode = 400;
        res.end();
        return;
      }
      if (file !== root && !file.startsWith(root + sep)) {
        res.statusCode = 404;
        res.end();
        return;
      }
      if (!existsSync(file) || !statSync(file).isFile()) { return next(); }
      res.setHeader("Content-Type", contentType(file));
      createReadStream(file).pipe(res);
    });
  },
});

/** The staged tree is wasm, the JS fallback, .bcmap CMaps and .pfb fonts;
 *  pdf.js fetches the last two as bytes, so anything unknown is a stream. */
function contentType(file: string): string {
  if (file.endsWith(".wasm")) { return "application/wasm"; }
  if (file.endsWith(".js") || file.endsWith(".mjs")) { return "text/javascript"; }
  return "application/octet-stream";
}

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [rawPdfjsRuntime(), solid()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
