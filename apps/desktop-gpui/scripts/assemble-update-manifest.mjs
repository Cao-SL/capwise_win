import { execFileSync } from "node:child_process";
import { open, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const [basePath, packageDir, outputPath, installerTag] = process.argv.slice(2);
if (!basePath || !packageDir || !outputPath || process.argv.length > 6) {
	throw new Error(
		"Usage: node assemble-update-manifest.mjs <mac-manifest> <signed-packages-dir> <output> [installer-release-tag]",
	);
}

const base = JSON.parse(await readFile(basePath, "utf8"));
const version = base.version;
if (typeof version !== "string" || !/^\d+\.\d+\.\d+$/.test(version)) {
	throw new Error("The base manifest must have a stable version");
}
const releaseTag = `v${version}`;
const previewPrefix = `${releaseTag}-platform-preview.`;
if (
	installerTag &&
	installerTag !== releaseTag &&
	(!installerTag.startsWith(previewPrefix) ||
		!/^\d+$/.test(installerTag.slice(previewPrefix.length)))
) {
	throw new Error("The installer release must match the manifest version");
}

const releaseUrl =
	process.env.CAPWISE_RELEASE_URL ??
	"https://github.com/joeseesun/capwise/releases/download";
const publicKey = fileURLToPath(
	new URL("../packaging/updater.pub", import.meta.url),
);
const gzip = Buffer.from([0x1f, 0x8b]);
const packages = [
	["darwin-aarch64", `Capwise_${version}_aarch64.app.tar.gz`, releaseTag, gzip],
	["darwin-x86_64", `Capwise_${version}_x86_64.app.tar.gz`, releaseTag, gzip],
	[
		"windows-x86_64",
		"Capwise-Windows-x64-Setup.exe",
		installerTag ?? releaseTag,
		Buffer.from("MZ"),
	],
	[
		"linux-x86_64-deb",
		"Capwise-linux-x86_64.deb",
		installerTag ?? releaseTag,
		Buffer.from("!<arch>\n"),
	],
];

const platforms = {};
for (const [platform, name, tag, magic] of packages) {
	const file = join(packageDir, name);
	const handle = await open(file, "r");
	try {
		const header = Buffer.alloc(magic.length);
		const { bytesRead } = await handle.read(header, 0, header.length, 0);
		if (bytesRead !== header.length || !header.equals(magic)) {
			throw new Error(`Unexpected package format for ${platform}: ${name}`);
		}
	} finally {
		await handle.close();
	}
	const signatureFile = `${file}.minisig`;
	execFileSync(
		"minisign",
		["-V", "-q", "-p", publicKey, "-m", file, "-x", signatureFile],
		{
			stdio: "inherit",
		},
	);
	const signature = (await readFile(signatureFile)).toString("base64");
	platforms[platform] = { url: `${releaseUrl}/${tag}/${name}`, signature };
}

const previousArm = base.platforms?.["darwin-aarch64"];
const verifiedArm = platforms["darwin-aarch64"];
if (
	previousArm?.url !== verifiedArm.url ||
	previousArm?.signature !== verifiedArm.signature
) {
	throw new Error(
		"The signed Apple Silicon archive does not match the base manifest",
	);
}

await writeFile(
	outputPath,
	`${JSON.stringify({ ...base, platforms }, null, 2)}\n`,
	{
		flag: "wx",
	},
);
