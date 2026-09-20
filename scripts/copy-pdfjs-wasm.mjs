// pdf.js decodes JPEG 2000 and JBIG2 in WebAssembly and loads those modules by
// name from `wasmUrl` at runtime, so they cannot go through Vite's hashed
// asset pipeline - they are copied verbatim into the static public dir.
// Without them a scanned magazine renders as blank white pages (PC World is
// JPX/JBIG2 throughout).
//
// The modules must also COMPILE in WebKit, or pdf.js silently falls back to
// its pure-JS decoder, which takes minutes per 600 dpi page (§19). pdfjs-dist
// 5.7+ builds openjpeg.wasm with relaxed SIMD, which JavaScriptCore rejects,
// so the version is pinned and this gate fails the build on an upgrade that
// brings the opcodes back.
import { cp, mkdir, readdir, readFile, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const pkg = join(root, "node_modules", "pdfjs-dist");
const from = join(pkg, "wasm");
const to = join(root, "public", "pdfjs");
// CMaps decode CJK text, the standard fonts stand in for unembedded ones;
// pdf.js fetches both by name at runtime, like the wasm modules.
const DIRS = ["cmaps", "standard_fonts"];

/** Number of relaxed-SIMD instructions (0xFD prefix, opcode 0x100-0x113) in
 *  the module's code section. */
export function relaxedSimdCount(bytes) {
  let pos = 8; // magic + version
  const leb = () => {
    let value = 0, shift = 0, byte;
    do { byte = bytes[pos++]; value |= (byte & 0x7f) << shift; shift += 7; } while (byte & 0x80);
    return value >>> 0;
  };
  while (pos < bytes.length) {
    const id = bytes[pos++];
    const size = leb();
    const end = pos + size;
    if (id === 10) {
      let hits = 0;
      for (let i = pos; i + 2 < end; i++) {
        if (bytes[i] === 0xfd && bytes[i + 1] >= 0x80 && bytes[i + 1] <= 0x93 && bytes[i + 2] === 0x02) { hits++; }
      }
      return hits;
    }
    pos = end;
  }
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  // Staging is wiped first: a module left over from another pdfjs-dist is
  // loaded by name just like the pinned one, which is the mismatch the pin
  // exists to prevent.
  await rm(to, { recursive: true, force: true });
  await mkdir(to, { recursive: true });
  const names = (await readdir(from)).filter((n) => !n.startsWith("LICENSE"));
  for (const name of names) {
    if (name.endsWith(".wasm")) {
      const hits = relaxedSimdCount(await readFile(join(from, name)));
      if (hits > 0) {
        console.error(`pdf.js: ${name} uses relaxed SIMD (${hits} instructions) - WebKit cannot compile it; keep pdfjs-dist on a pin whose wasm does not (see scripts/copy-pdfjs-wasm.mjs)`);
        process.exit(1);
      }
    }
    await cp(join(from, name), join(to, name));
  }
  let extra = 0;
  for (const dir of DIRS) {
    await cp(join(pkg, dir), join(to, dir), { recursive: true });
    extra += (await readdir(join(to, dir))).length;
  }
  console.log(
    `pdf.js: staged ${names.length} runtime files and ${extra} ${DIRS.join("/")} files in public/pdfjs`,
  );
}
