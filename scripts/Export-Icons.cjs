const fs = require('node:fs');
const path = require('node:path');
const { Resvg } = require('../target/icon-tools/node_modules/@resvg/resvg-js');

const directory = path.join(__dirname, '..', 'src-tauri', 'icons');
const source = fs.readFileSync(path.join(directory, 'icon.svg'));
const sizes = [16, 24, 32, 48, 64, 128, 256];
const images = sizes.map(size => new Resvg(source, {
  fitTo: { mode: 'width', value: size },
  font: { loadSystemFonts: false },
}).render());
const pngs = images.map(image => image.asPng());
fs.writeFileSync(path.join(directory, 'icon.png'), pngs.at(-1));

const header = Buffer.alloc(6 + sizes.length * 16);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
sizes.forEach((size, index) => {
  const entry = 6 + index * 16;
  header[entry] = header[entry + 1] = size === 256 ? 0 : size;
  header.writeUInt16LE(1, entry + 4);
  header.writeUInt16LE(32, entry + 6);
  header.writeUInt32LE(pngs[index].length, entry + 8);
  header.writeUInt32LE(offset, entry + 12);
  offset += pngs[index].length;
});
fs.writeFileSync(path.join(directory, 'icon.ico'), Buffer.concat([header, ...pngs]));
fs.writeFileSync(path.join(directory, 'tray.rgba'), images[2].pixels);
