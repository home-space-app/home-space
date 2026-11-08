# Turns the canvas artboards (source/*.dc.html) into standalone pages (html/*.html).
# Usage: python3 convert.py ../source ..
import re, sys, pathlib, shutil
src = pathlib.Path(sys.argv[1]); out = pathlib.Path(sys.argv[2])
(out/"html").mkdir(parents=True, exist_ok=True)
for css in src.glob("*.css"): shutil.copy(css, out/"html"/css.name)
# Main.dc.html is the canvas entry; as a page it is the Files screen.
def page(stem): return "Files" if stem == "Main" else stem
for f in sorted(src.glob("*.dc.html")):
    t = f.read_text()
    helmet = re.search(r"<helmet>(.*?)</helmet>", t, re.S).group(1)
    t = re.sub(r"<helmet>.*?</helmet>\n?", "", t, flags=re.S)
    t = t.replace('<script src="./support.js"></script>\n', "")
    t = t.replace("</head>", helmet.strip() + "\n</head>")
    t = re.sub(r"<x-dc>\n?|</x-dc>\n?", "", t)
    t = re.sub(r'<script type="text/x-dc".*?</script>\n?', "", t, flags=re.S)
    t = t.replace(" {{cls}}", "")
    t = re.sub(r'href="([A-Za-z]+)\.dc\.html"', lambda m: f'href="{page(m.group(1))}.html"', t)
    assert "{{" not in t, f
    name = page(f.name.replace(".dc.html", "")) + ".html"
    (out/"html"/name).write_text(t)
    print(name)
