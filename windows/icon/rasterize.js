#!/usr/bin/env node
// Regenerate the NotifyIcon rasters from windows/tray-icon.svg.
//
//   node windows/icon/rasterize.js
//
// It serves render.html on a loopback port, prints the URL, and waits for the
// browser to post back one anti-aliased PNG per size. The PNGs are decoded here
// (no npm packages: a PNG is zlib plus one filter byte per row) into the raw
// RGBA files `src/tray/icon.rs` embeds, RGB forced to black so no colour fringe
// can appear on translucent edge pixels. Base64 never travels through a
// terminal or a chat window: hand-copied PNG data corrupted twice before this
// script existed.
const fs = require("fs");
const http = require("http");
const path = require("path");
const zlib = require("zlib");

const ROOT = path.resolve(__dirname, "..", "..");
const SVG = path.join(ROOT, "windows", "tray-icon.svg");
const PREVIEW = path.join(ROOT, "windows", "tray-icon.png");
const PREVIEW_SIZE = "32";
const PORT = Number(process.env.PORT || 5180);
const IDLE_TIMEOUT_MS = 5 * 60 * 1000;

const rasterPath = (size) => path.join(ROOT, "windows", `tray-icon-${size}.rgba`);

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const paeth = (a, b, c) => {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
};

/** Decode an 8-bit RGBA PNG into raw pixels. */
function decodePng(png) {
  if (png.readUInt32BE(0) !== 0x89504e47) throw new Error("not a PNG");
  let pos = 8;
  let width = 0;
  let height = 0;
  const idat = [];
  while (pos < png.length) {
    const len = png.readUInt32BE(pos);
    const type = png.toString("ascii", pos + 4, pos + 8);
    const data = png.subarray(pos + 8, pos + 8 + len);
    if (crc32(png.subarray(pos + 4, pos + 8 + len)) !== png.readUInt32BE(pos + 8 + len)) {
      throw new Error("CRC mismatch in " + type);
    }
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      if (data[8] !== 8 || data[9] !== 6) throw new Error("expected 8-bit RGBA");
    }
    if (type === "IDAT") idat.push(data);
    pos += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const out = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const src = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const row = out.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : Buffer.alloc(stride);
    for (let i = 0; i < stride; i++) {
      const a = i >= 4 ? row[i - 4] : 0;
      const b = prev[i];
      const c = i >= 4 ? prev[i - 4] : 0;
      let v = src[i];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) v += paeth(a, b, c);
      else if (filter !== 0) throw new Error("bad filter " + filter);
      row[i] = v & 0xff;
    }
  }
  return { width, height, out };
}

function writeRasters(table) {
  const lines = [];
  for (const [size, b64] of Object.entries(table)) {
    const png = Buffer.from(b64, "base64");
    const { width, height, out } = decodePng(png);
    if (width !== Number(size) || height !== Number(size)) {
      throw new Error(`size mismatch for ${size}: ${width}x${height}`);
    }
    let opaque = 0;
    let partial = 0;
    for (let i = 0; i < out.length; i += 4) {
      out[i] = 0;
      out[i + 1] = 0;
      out[i + 2] = 0;
      if (out[i + 3] === 255) opaque++;
      else if (out[i + 3] > 0) partial++;
    }
    fs.writeFileSync(rasterPath(size), out);
    if (size === PREVIEW_SIZE) fs.writeFileSync(PREVIEW, png);
    lines.push(
      `${size} px: ${opaque} opaque, ${partial} anti-aliased pixels -> ${path.relative(ROOT, rasterPath(size))}`,
    );
  }
  return lines.join("\n");
}

const server = http.createServer((req, res) => {
  if (req.method === "GET" && (req.url === "/" || req.url === "/render.html")) {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(fs.readFileSync(path.join(__dirname, "render.html")));
    return;
  }
  if (req.method === "GET" && req.url === "/tray-icon.svg") {
    res.writeHead(200, { "content-type": "image/svg+xml" });
    res.end(fs.readFileSync(SVG));
    return;
  }
  if (req.method === "POST" && req.url === "/rasters") {
    let body = "";
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => {
      try {
        const report = writeRasters(JSON.parse(body));
        res.writeHead(200, { "content-type": "text/plain" });
        res.end("done\n" + report);
        console.log(report);
        console.log("Rebuild the tray: cargo build --bin ai-usagebar-tray");
      } catch (error) {
        res.writeHead(500, { "content-type": "text/plain" });
        res.end("failed: " + error.message);
        console.error("failed:", error.message);
      }
      setTimeout(() => server.close(), 200);
    });
    return;
  }
  res.writeHead(404);
  res.end();
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(
    `Open http://127.0.0.1:${PORT}/ in a browser; it renders ${path.relative(ROOT, SVG)} and posts the rasters back.`,
  );
});
setTimeout(() => {
  console.log("No rasters received; giving up.");
  server.close();
}, IDLE_TIMEOUT_MS).unref();
