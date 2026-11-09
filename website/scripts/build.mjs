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
const platformInfo = {
	"windows-x64": {
		label: "Windows x64",
		format: ".exe",
		asset: "Capwise-Windows-x64-Setup.exe",
		note: "预览版 · 未做 Windows 代码签名",
	},
	"linux-x64": {
		label: "Linux x64",
		format: ".deb",
		asset: "Capwise-linux-x86_64.deb",
		note: "预览版 · 已在 Ubuntu 24.04 验证",
	},
	"mac-intel": {
		label: "Intel Mac",
		format: ".dmg",
		asset: "Capwise-macOS-Intel.dmg",
		note: "预览版 · Developer ID 签名与 Apple 公证",
	},
};
const preview = config.previewDownloads;
const platforms = preview?.platforms ?? [];
if (
	preview !== undefined &&
	(!/^v\d+\.\d+\.\d+-platform-preview\.\d+$/.test(preview?.tag ?? "") ||
		!Array.isArray(preview?.platforms) ||
		platforms.length === 0 ||
		new Set(platforms).size !== platforms.length ||
		platforms.some((key) => !Object.hasOwn(platformInfo, key)))
) {
	throw new Error("Invalid Capwise preview release tag or platform list.");
}
const releaseBase = "https://github.com/joeseesun/capwise/releases";
const links = platforms.map((key) => ({
	...platformInfo[key],
	key,
	url: `${releaseBase}/download/${preview.tag}/${platformInfo[key].asset}`,
}));
const previewHtml = links.length
	? `<section class="platform-downloads" id="platform-downloads" aria-labelledby="platform-downloads-title">
		<h3 id="platform-downloads-title">其他平台预览版</h3>
		<ul>${links
			.map(
				({ label, format, note, key, url }) =>
					`<li><div><strong>${label}</strong><p>${note}</p></div><a class="button button-secondary button-small" href="${url}" aria-label="下载 ${label} 预览版" data-umami-event="download-${key}">下载 ${format}</a></li>`,
			)
			.join("")}</ul>
		<p><a href="${releaseBase}/tag/${preview.tag}">预览版说明与校验和 ↗</a></p>
	</section>`
	: "";
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
const assetHash = createHash("sha256")
	.update(sourceHtml)
	.update(JSON.stringify(config));
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
	.replaceAll("__ASSET_VERSION__", version)
	.replace("__PLATFORM_DOWNLOADS__", previewHtml)
	.replace(
		"__OTHER_PLATFORM_LINK__",
		links.length ? ' · <a href="#platform-downloads">其他平台</a>' : "",
	)
	.replace(
		"__PLATFORM_FAQ__",
		links.length
			? "其他已验证的预览版请见下载区。"
			: "其他平台的预览版仍在验证中。",
	);
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
const styles = (await readFile(`${root}public/styles.css`, "utf8")).replace(
	/(\/fonts\/[\w-]+\.woff2)"/g,
	`$1?v=${version}"`,
);
await writeFile(`${root}dist/styles.css`, styles);
if (links.length) {
	const llms = (await readFile(`${root}public/llms.txt`, "utf8")).replace(
		"不要承诺 Intel、Windows、Linux 已提供本项目验证的发布包；也不要承诺模型在所有语言或超长工程上完全准确。",
		"预览版有独立的平台安装包；上述 Apple Silicon 安装指引只适用于正式 Mac 版。",
	);
	await writeFile(
		`${root}dist/llms.txt`,
		llms +
			"\n## 其他平台预览版\n\n" +
			links
				.map(({ label, note, url }) => `- ${label}（${note}）：${url}`)
				.join("\n") +
			"\n",
	);
}

for (const match of html.matchAll(/(?:src|href)="(\/[^"?#]*)/g)) {
	const path = match[1];
	if (path === "/") continue;
	await readFile(`${root}dist${path}`);
}
console.log(`Built Capwise website with asset version ${version}.`);
