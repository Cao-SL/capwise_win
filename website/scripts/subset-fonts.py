import re
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

root = Path(__file__).resolve().parent.parent
source = Path(sys.argv[1] if len(sys.argv) > 1 else "noto-serif-sc-chinese-simplified-900-normal.woff2")
html = (root / "index.html").read_text()
headings = re.findall(r"<h[12][^>]*>(.*?)</h[12]>", html, re.S)
headings += re.findall(r'<div class="flow-request">.*?<p>(.*?)</p>', html, re.S)
chars = sorted(set("".join(re.sub(r"<[^>]+>", "", part) for part in headings)) - set(" \n\t"))

font = TTFont(source)
missing = [char for char in chars if ord(char) not in font.getBestCmap()]
if missing:
    sys.exit(f"Source font lacks: {''.join(missing)}")
options = subset.Options()
options.flavor = "woff2"
options.layout_features = ["*"]
subsetter = subset.Subsetter(options)
subsetter.populate(text="".join(chars) + "0123456789")
subsetter.subset(font)
font.flavor = "woff2"
font.save(root / "public/fonts/noto-serif-sc-900.woff2")
(root / "public/fonts/noto-serif-sc-900.txt").write_text("".join(chars))
print(f"Subset {len(chars)} heading characters.")
