import { createHash } from "node:crypto";
import {
	cp,
	mkdir,
	readdir,
	readFile,
	rm,
	stat,
	writeFile,
} from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const config = JSON.parse(await readFile(`${root}site.config.json`, "utf8"));
if (!/^[a-f\d]{8}(-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(config.umamiWebsiteId)) {
	throw new Error(
		"A registered Umami website ID is required to build production.",
	);
}
const sourceHtml = await readFile(`${root}index.html`, "utf8");
const headingText = [
	...sourceHtml.matchAll(/<h[12][^>]*>([\s\S]*?)<\/h[12]>/g),
	...sourceHtml.matchAll(
		/<div class="flow-request">[\s\S]*?<p>([\s\S]*?)<\/p>/g,
	),
]
	.map((match) => match[1].replace(/<[^>]+>/g, ""))
	.join("");
const subsetChars = new Set(
	await readFile(`${root}public/fonts/noto-serif-sc-900.txt`, "utf8"),
);
const missingGlyphs = [
	...new Set([...headingText].filter((char) => !/\s/.test(char))),
].filter((char) => !subsetChars.has(char));
if (missingGlyphs.length) {
	throw new Error(
		`Heading font subset is missing ${missingGlyphs.join("")}; run scripts/subset-fonts.py.`,
	);
}
const assetHash = createHash("sha256").update(sourceHtml);
for (const path of (
	await readdir(`${root}public`, { recursive: true })
).sort()) {
	const absolute = `${root}public/${path}`;
	if (!(await stat(absolute)).isFile()) continue;
	assetHash.update(path).update(await readFile(absolute));
}
const version = assetHash.digest("hex").slice(0, 12);
await rm(`${root}dist`, { recursive: true, force: true });
await mkdir(`${root}dist`, { recursive: true });
await cp(`${root}public`, `${root}dist`, { recursive: true });
let html = sourceHtml;
html = html
	.replaceAll("__UMAMI_ID__", config.umamiWebsiteId)
	.replaceAll("__ASSET_VERSION__", version);
const jsonld = html.match(
	/<script type="application\/ld\+json">([\s\S]*?)<\/script>/,
)?.[1];
if (!jsonld) throw new Error("Product structured data is missing.");
JSON.parse(jsonld);
const hash = createHash("sha256").update(jsonld).digest("base64");
const headers = (await readFile(`${root}public/_headers`, "utf8")).replaceAll(
	"__JSONLD_HASH__",
	hash,
);
const worker = (await readFile(`${root}public/sw.js`, "utf8")).replaceAll(
	"__ASSET_VERSION__",
	version,
);
await writeFile(`${root}dist/index.html`, html);
await writeFile(`${root}dist/_headers`, headers);
await writeFile(`${root}dist/sw.js`, worker);
for (const match of html.matchAll(/(?:src|href)="(\/[^"?#]*)/g)) {
	const path = match[1];
	if (path === "/") continue;
	await readFile(`${root}dist${path}`);
}
console.log(`Built Capwise website with asset version ${version}.`);
