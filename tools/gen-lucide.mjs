// Generates data/lucide.json from the lucide-react package used by the
// desktop app: icon name -> inner SVG markup (24x24, stroke-based), plus
// aliases. Run from the repository root:
//   node plugins/import-pen/tools/gen-lucide.mjs
// Lucide is ISC-licensed; see data/LUCIDE_LICENSE.
import { readFileSync, readdirSync, writeFileSync, copyFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const pkg = join(here, "../../../apps/desktop/node_modules/lucide-react");
const dir = join(pkg, "dist/esm/icons");
const esc = (v) => String(v).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
const icons = {};
const aliases = {};
for (const f of readdirSync(dir).sort()) {
  if (!f.endsWith(".mjs")) continue;
  const src = readFileSync(join(dir, f), "utf8");
  const m = src.match(/const __iconData = (\{[\s\S]*?\n\});/);
  if (!m) continue;
  const data = Function(`return (${m[1]});`)();
  icons[data.name] = data.node
    .map(([tag, attrs]) => {
      const a = Object.entries(attrs)
        .filter(([k]) => k !== "key")
        .map(([k, v]) => `${k}="${esc(v)}"`)
        .join(" ");
      return `<${tag} ${a}/>`;
    })
    .join("");
  for (const a of data.aliases ?? []) aliases[a] = data.name;
}
const version = JSON.parse(readFileSync(join(pkg, "package.json"), "utf8")).version;
writeFileSync(
  join(here, "../data/lucide.json"),
  JSON.stringify({ version, icons, aliases }) + "\n",
);
copyFileSync(join(pkg, "LICENSE"), join(here, "../data/LUCIDE_LICENSE"));
console.log(`lucide ${version}: ${Object.keys(icons).length} icons, ${Object.keys(aliases).length} aliases`);
