const tours = {
	captions: {
		image: "captions.webp",
		title: "字幕不只是生成，还要改得顺、看得清。",
		description: "内容、翻译、样式各有入口，校对文字与调整外观互不打扰。",
		alt: "灵录字幕内容与双语视频预览的真实界面",
	},
	assets: {
		image: "assets.webp",
		title: "素材放进一个库，下次创作接着用。",
		description:
			"项目素材、共享素材与在线搜索分开管理，图片、音乐、视频各有位置。",
		alt: "灵录素材库与缩略图网格的真实界面",
	},
	demo: {
		image: "demo-frame.webp",
		title: "好看的工程，打开就能继续编辑。",
		description:
			"原生导出的演示画面。下载工程，亲手试试文字、双语字幕、缩放和配乐。",
		alt: "灵感成片演示工程的真实导出画面，不是应用界面截图",
	},
};

const tabs = [...document.querySelectorAll("[data-tour]")];
const image = document.querySelector("#tour-image");
const title = document.querySelector("#tour-title");
const description = document.querySelector("#tour-description");
const panel = document.querySelector("#tour-panel");

function selectTour(tab) {
	const tour = tours[tab.dataset.tour];
	if (!tour) return;
	for (const item of tabs) {
		const selected = item === tab;
		item.setAttribute("aria-selected", String(selected));
		item.tabIndex = selected ? 0 : -1;
	}
	image.src = `/assets/${tour.image}`;
	image.alt = tour.alt;
	title.textContent = tour.title;
	description.textContent = tour.description;
	panel.setAttribute("aria-labelledby", tab.id);
}

for (const [index, tab] of tabs.entries()) {
	tab.addEventListener("click", () => selectTour(tab));
	tab.addEventListener("keydown", (event) => {
		let next;
		if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
		if (event.key === "ArrowLeft")
			next = (index + tabs.length - 1) % tabs.length;
		if (event.key === "Home") next = 0;
		if (event.key === "End") next = tabs.length - 1;
		if (next === undefined) return;
		event.preventDefault();
		selectTour(tabs[next]);
		tabs[next].focus();
	});
}

const supportDialog = document.querySelector("#support-dialog");
const supportTitle = document.querySelector("#support-title");
const supportImage = document.querySelector("#support-image");
const supportDescription = document.querySelector("#support-description");
for (const button of document.querySelectorAll("[data-modal]")) {
	button.addEventListener("click", () => {
		const reward = button.dataset.modal === "reward";
		supportTitle.textContent = reward
			? "喜欢灵录，请我喝杯咖啡"
			: "关注向阳乔木";
		supportImage.src = reward
			? "/assets/reward-qr.png"
			: "/assets/follow-qr.jpg";
		supportImage.alt = reward
			? "向阳乔木打赏二维码"
			: "向阳乔木推荐看公众号二维码";
		supportDescription.textContent = reward
			? "自愿支持，感谢你让开源创作继续。"
			: "微信公众号「向阳乔木推荐看」";
		supportDialog.showModal();
	});
}

document.querySelector("#enlarge-screenshot").addEventListener("click", () => {
	const large = document.querySelector("#large-screenshot");
	large.src = image.src;
	large.alt = image.alt;
	document.querySelector("#image-dialog").showModal();
});
for (const dialog of document.querySelectorAll("dialog")) {
	dialog
		.querySelector("[data-close-dialog]")
		.addEventListener("click", () => dialog.close());
	dialog.addEventListener("click", (event) => {
		if (event.target !== dialog) return;
		const rect = dialog.getBoundingClientRect();
		if (
			event.clientX < rect.left ||
			event.clientX > rect.right ||
			event.clientY < rect.top ||
			event.clientY > rect.bottom
		)
			dialog.close();
	});
}

document.querySelector("#copy-agent").addEventListener("click", async () => {
	const prompt = document.querySelector("#agent-prompt");
	const status = document.querySelector("#copy-status");
	try {
		await navigator.clipboard.writeText(prompt.textContent.trim());
		status.textContent = "已复制，粘贴给你的 Agent 即可。";
	} catch {
		const selection = window.getSelection();
		const range = document.createRange();
		range.selectNodeContents(prompt);
		selection.removeAllRanges();
		selection.addRange(range);
		status.textContent = "浏览器未允许自动复制，提示词已选中，请手动复制。";
	}
});

const presetCard = document.querySelector(".preset-card");
if (
	presetCard &&
	"IntersectionObserver" in window &&
	!matchMedia("(prefers-reduced-motion: reduce)").matches
) {
	const steps = [...presetCard.querySelectorAll(".preset-steps li")];
	presetCard.classList.add("is-armed");
	const observer = new IntersectionObserver(
		(entries) => {
			if (!entries.some((entry) => entry.isIntersecting)) return;
			observer.disconnect();
			steps.forEach((step, index) => {
				setTimeout(() => step.classList.add("is-done"), 250 + index * 260);
			});
			setTimeout(
				() => presetCard.classList.add("is-complete"),
				250 + steps.length * 260,
			);
		},
		{ threshold: 0.45 },
	);
	observer.observe(presetCard);
}

const header = document.querySelector(".site-header");
const updateHeader = () =>
	header.classList.toggle("is-scrolled", window.scrollY > 8);
updateHeader();
window.addEventListener("scroll", updateHeader, { passive: true });

if (
	"IntersectionObserver" in window &&
	!matchMedia("(prefers-reduced-motion: reduce)").matches
) {
	const revealObserver = new IntersectionObserver(
		(entries) => {
			for (const entry of entries) {
				if (!entry.isIntersecting) continue;
				entry.target.classList.add("is-visible");
				revealObserver.unobserve(entry.target);
			}
		},
		{ rootMargin: "0px 0px -8% 0px" },
	);
	for (const element of document.querySelectorAll(
		"main section:not(.hero) .section-heading, .story-copy, .auto-copy, .story-media, .cut-demo, .assistant-flow, .preset-card, .tour-panel, .capability-grid article, .local-details article, .install-steps, .agent-card",
	)) {
		if (element.getBoundingClientRect().top < window.innerHeight) continue;
		element.classList.add("reveal");
		revealObserver.observe(element);
	}
}

if ("serviceWorker" in navigator && location.protocol === "https:") {
	navigator.serviceWorker.register("/sw.js").catch(() => {});
}
