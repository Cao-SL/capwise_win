import argparse
import copy
import json
import math
from pathlib import Path
import shutil
import subprocess
import uuid

from PIL import Image, ImageDraw, ImageFont, ImageFilter


parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--template", type=Path, required=True)
parser.add_argument("--music", type=Path, required=True)
args = parser.parse_args()
root = args.output
root.mkdir(parents=True, exist_ok=False)
assets = root / "content"
(assets / "segments/segment-0").mkdir(parents=True)
(assets / "images").mkdir()
(assets / "audio").mkdir()
(root / "screenshots").mkdir()
fonts = Path.home() / "Library/Fonts"
cn = fonts / "NotoSansCJKsc-Regular.otf"
bold = fonts / "NotoSansCJKsc-Bold.otf"
latin = Path(__file__).resolve().parents[2] / "apps/desktop-gpui/assets/fonts/Geist.ttf"
ink = "#17362F"
muted = "#71827A"
paper = "#F8FAF5"


def text(draw, xy, value, size=24, fill=ink, weight=False, english=False):
    font = ImageFont.truetype(str(latin if english else bold if weight else cn), size)
    draw.text(xy, value, font=font, fill=fill, stroke_width=0)


def rounded(draw, box, color, radius=24, outline=None, width=1):
    draw.rounded_rectangle(box, radius, fill=color, outline=outline, width=width)


def star(draw, x, y, radius, fill):
    points = []
    for i in range(32):
        angle = i * math.pi / 16
        r = radius if i % 2 == 0 else radius * 0.77
        points.append((x + math.cos(angle) * r, y + math.sin(angle) * r))
    draw.polygon(points, fill=fill)


def base(chapter):
    image = Image.new("RGB", (1920, 1080), paper)
    d = ImageDraw.Draw(image)
    rounded(d, (62, 45, 104, 87), ink, 12)
    text(d, (74, 45), "n", 31, "#E2F8BD", english=True)
    text(d, (122, 46), "nova", 30, english=True)
    text(d, (240, 50), "灵感档案", 21, muted)
    text(d, (1350, 54), "创作手记", 20, muted)
    text(d, (1503, 54), "灵感收藏", 20, muted)
    rounded(d, (1663, 40, 1856, 94), ink, 27)
    text(d, (1700, 48), "开始创作  ↗", 22, "#F3F7EC")
    d.line((64, 126, 1856, 126), "#DDE5DB", 2)
    text(d, (86, 174), f"THE CREATIVE JOURNAL   /   0{chapter}", 20, muted, english=True)
    for i, label in enumerate(["捕捉灵感", "理清思路", "分享作品"]):
        x = 86 + i * 590
        d.line((x, 847, x + 520, 847), "#D5DED2", 2)
        text(d, (x, 873), f"0{i+1}", 24, muted, english=True)
        text(d, (x + 65, 869), label, 28, ink, True)
        text(d, (x + 65, 922), ["把脑海中的闪光，先留下来", "让复杂的过程，一眼就懂", "让好想法，抵达更多人"][i], 20, muted)
    text(d, (86, 1017), "NOVA  /  ORIGINAL DEMO CONTENT", 15, muted, english=True)
    text(d, (1580, 1010), "灵录演示 · 原创示例", 17, muted)
    return image


def artwork(chapter):
    im = base(chapter)
    d = ImageDraw.Draw(im)
    rounded(d, (1060, 235, 1835, 770), "#E3EEDB", 48)
    if chapter == 1:
        shadow = Image.new("RGBA", im.size)
        sd = ImageDraw.Draw(shadow)
        rounded(sd, (1165, 315, 1700, 710), (30, 60, 35, 28), 24)
        im = Image.alpha_composite(im.convert("RGBA"), shadow.filter(ImageFilter.GaussianBlur(24)))
        d = ImageDraw.Draw(im)
        card = Image.new("RGBA", (540, 385), "#FFFDF4")
        cd = ImageDraw.Draw(card)
        cd.ellipse((75, 30, 455, 350), fill="#CFE99A")
        cd.ellipse((142, 65, 402, 325), fill="#1D5144")
        cd.ellipse((180, 103, 365, 288), fill="#E5F2C5")
        cd.ellipse((211, 135, 333, 257), fill="#8CB692")
        cd.arc((35, 121, 500, 280), 0, 360, fill="#FFFFFF", width=8)
        star(cd, 415, 80, 45, "#E6E986")
        card = card.rotate(-8, Image.Resampling.BICUBIC, expand=True)
        im.alpha_composite(card, (1110, 278))
        d = ImageDraw.Draw(im)
        rounded(d, (1570, 638, 1800, 718), ink, 18)
        text(d, (1601, 653), "一个好点子", 26, "#F0F6DF", True)
        text(d, (1100, 249), "IDEA NO. 001", 17, muted, english=True)
    elif chapter == 2:
        for i, (label, small, color) in enumerate([
            ("捕捉", "CAPTURE", "#FDFEF7"),
            ("梳理", "CONNECT", "#1D5144"),
            ("表达", "CREATE", "#E5E998"),
        ]):
            x, y = 1120 + i * 205, 350 + (i % 2) * 95
            if i < 2:
                d.line((x + 135, y + 65, x + 240, 510 if i == 0 else 420), "#78917D", 4)
            rounded(d, (x, y, x + 175, y + 170), color, 30)
            text(d, (x + 55, y + 27), label, 31, paper if i == 1 else ink, True)
            text(d, (x + 35, y + 108), small, 18, "#D6E4D5" if i == 1 else muted, english=True)
        rounded(d, (1195, 675, 1708, 726), "#D4E6C5", 25)
        text(d, (1241, 679), "让每一步，都有清晰的方向", 23, ink)
    else:
        for i, color in enumerate(["#C4DAB2", "#AAC9A0", "#1D5144"]):
            x, y = 1180 + i * 97, 280 + i * 58
            rounded(d, (x, y, x + 365, y + 310), color, 32)
        d.ellipse((1450, 453, 1570, 573), fill="#EBEEAA")
        d.polygon([(1495, 480), (1495, 548), (1540, 514)], fill=ink)
        text(d, (1412, 620), "READY TO SHARE", 18, "#E8F2D8", english=True)
        star(d, 1720, 315, 68, "#E5E998")
        text(d, (1100, 249), "FROM IDEA TO IMPACT", 17, muted, english=True)
    return im.convert("RGB")


for i in range(1, 4):
    artwork(i).save(assets / f"images/chapter-{i}.png")
concat = root.parent / f"{root.stem}-sources.txt"
with concat.open("w") as f:
    for i in range(1, 4):
        f.write(f"file '{assets / f'images/chapter-{i}.png'}'\nduration 7\n")
    f.write(f"file '{assets / 'images/chapter-3.png'}'\n")
subprocess.run([
    "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0",
    "-i", str(concat), "-t", "21", "-vf", "fps=30,format=yuv420p", "-c:v", "libx264",
    "-preset", "medium", "-crf", "16", "-movflags", "+faststart",
    str(assets / "segments/segment-0/display.mp4"),
], check=True)
subprocess.run([
    "ffmpeg", "-hide_banner", "-loglevel", "error", "-ss", "8", "-i", str(args.music),
    "-t", "24", "-c:a", "libmp3lame", "-b:a", "192k",
    str(assets / "audio/creative-flow.mp3"),
], check=True)
config = json.loads((args.template / "project-config.json").read_text())
meta = json.loads((args.template / "recording-meta.json").read_text())
meta["pretty_name"] = "灵感成片 · Make Ideas Visible"
meta["segments"] = [{"display": {"path": "content/segments/segment-0/display.mp4", "fps": 30, "start_time": 0.0}}]
config["background"]["source"] = {"type": "color", "value": [226, 235, 221], "alpha": 255}
config["background"].update(padding=3.0, rounding=3.0, shadow=35.0, advancedShadow={"size": 8.0, "opacity": 30.0, "blur": 16.0})
config["camera"]["hide"] = True
config["cursor"]["hide"] = True
config["audio"]["improve"] = False
config["clips"] = [config["clips"][0]]
config["annotations"] = []
config["colorCorrection"]["screen"]["preset"] = "none"
template_text = copy.deepcopy(config["timeline"]["textSegments"][0])
template_text.pop("titleCard", None)
template_text.update(layout="overlay", color=ink, backgroundStyle="box", fontFamily="Noto Sans SC", fontSize=72, align="left", animationIn="slideUp", animationOut="fade", animationInDuration=0.5, animationOutDuration=0.22, shadow=0.0)
texts = []


def title(value, start, end, x, y, size, width=0.46, color=ink, track=1, font="Noto Sans SC"):
    t = copy.deepcopy(template_text)
    t.update(content=value, start=start, end=end, track=track, center={"x": x, "y": y}, size={"x": width, "y": 0.23}, fontSize=size, fontFamily=font, color=color, fontWeight=700 if font == "Noto Sans SC" else 400)
    texts.append(t)
    return t


intro = title("把灵感，变成作品。", 0, 1, 0.5, 0.5, 86, 0.85, ink, 0)
intro.update(layout="fullscreen", align="center", animationIn="none", animationOut="none", animationInDuration=0.0, animationOutDuration=0.0)
intro["titleCard"] = {"id": uuid.uuid4().hex, "role": "intro", "template": "editorial", "templateVersion": 1, "kicker": "CAPWISE / 灵录", "subtitle": "Make ideas visible.", "background": {"type": "solid", "color": "#EEF3E7"}}
for i, (headline, subline) in enumerate([
    ("好想法，\n值得被看见。", "Every idea deserves a good story."),
    ("把复杂，\n讲得更简单。", "Make every step clear."),
    ("让作品，\n抵达更多人。", "Create once. Share your world."),
]):
    start, end = 1 + i * 7, 8 + i * 7
    title(headline, start, end, 0.285, 0.405, 78, 0.44)
    title(subline, start + 0.2, end, 0.283, 0.60, 27, 0.44, muted, 2, "Arial")
outro = title("录下来，讲清楚。", 22, 24, 0.5, 0.5, 90, 0.85, "#F1F5E7", 0)
outro.update(layout="fullscreen", align="center", animationIn="none", animationOut="none", animationInDuration=0.0, animationOutDuration=0.0)
outro["titleCard"] = {"id": uuid.uuid4().hex, "role": "outro", "template": "hero", "templateVersion": 1, "kicker": "灵录 CAPWISE", "subtitle": "capwise.qiaomu.ai", "background": {"type": "solid", "color": ink}}
lines = [
    (1.5, 4.3, "一个好想法，值得一次清晰的表达", "A good idea deserves a clear story."),
    (4.6, 7.6, "从捕捉灵感开始，让创作自然发生", "Start with a spark. Let creativity flow."),
    (8.5, 11.4, "把复杂的过程，拆成简单的步骤", "Turn a complex process into simple steps."),
    (11.8, 14.6, "用字幕和重点标注，让观众跟得上", "Help your audience follow with captions and callouts."),
    (15.2, 18.0, "为作品加上自己的风格", "Give your work a style of its own."),
    (18.4, 21.5, "准备好了，就把好想法分享出去", "When you are ready, share your idea with the world."),
]
captions = []
translations = {}
for i, (start, end, value, en) in enumerate(lines):
    ident = f"creative-{i+1}"
    captions.append({"id": ident, "start": start, "end": end, "text": value, "words": []})
    translations[ident] = {"sourceText": value, "text": en, "language": "English"}
settings = config["captions"]["settings"]
settings.update(enabled=True, displayMode="bilingual", translations=translations, font="Noto Sans SC", size=33, color="#FFFFFF", backgroundColor="#17362F", backgroundOpacity=88, position="bottom", outline=False, outlineWidth=0.0, shadow=False, shadowOpacity=0, preset="soft-black", translationStyle={"font": "Geist", "size": 22}, translationAppearance={"color": "#E3ECD9", "outline": False, "outlineWidth": 0.0, "outlineColor": ink, "shadow": False, "shadowColor": ink, "shadowOpacity": 0, "shadowSize": 0.0, "fontWeight": 500, "uppercase": False}, translationGap=7, bottomMargin=22, manualPosition=None, fadeDuration=0.15, lingerDuration=0.1)
config["captions"]["sourceTimed"] = False
config["captions"]["segments"] = captions
config["timeline"] = {
    "segments": [{"recordingSegment": 0, "timescale": 1.0, "start": 0.0, "end": 21.0}],
    "zoomSegments": [{"start": 9.2, "end": 12.5, "amount": 1.12, "mode": {"manual": {"x": 0.71, "y": 0.48}}, "glideDirection": "none", "glideSpeed": 0.5, "instantAnimation": False, "edgeSnapRatio": 0.25}],
    "textSegments": texts,
    "captionSegments": captions,
    "audioSegments": [{"start": 0.0, "end": 24.0, "track": 0, "path": "content/audio/creative-flow.mp3", "name": "Creative flow · 原创配乐", "enabled": True, "trimStart": 0.0, "volumeDb": -18.0, "ducking": True, "fadeIn": 0.8, "fadeOut": 1.8, "duration": 24.0}],
}
config["overlayOrder"] = [{"kind": "text", "track": i} for i in range(3)]
for name, value in [("recording-meta.json", meta), ("project-config.json", config)]:
    (root / name).write_text(json.dumps(value, ensure_ascii=False, indent=2))
artwork(1).save(root / "screenshots/display.jpg", quality=90)
(root / "DEMO-NOTES.md").write_text("# 灵感成片 · Make Ideas Visible\n\n24 秒原创演示工程。NOVA 是虚构示例内容，不是真实第三方产品。\n\n可编辑：片头与片尾、三个章节的中文标题和英文副标题、六条双语字幕、缩放、配乐音量与淡入淡出。字幕是人工编写的演示文案，不包含逐词语音对齐。\n\n配乐来自灵录内置的 friendly-welcome，由项目维护者提供的 AI 原创音乐；本工程截取其中 24 秒，默认 -18 dB。未使用麦克风或私人录屏。\n\n打开工程不会主动开始播放。按播放后可以试听；标题可直接选中编辑。\n")
print(root)
