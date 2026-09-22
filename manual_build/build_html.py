# /// script
# requires-python = ">=3.10"
# ///
import html, os, re

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS = os.path.join(HERE, "assets")

CSS = """
:root{
  --ink:#1a1f2b; --ink2:#3c4453; --muted:#6b7385; --faint:#9aa3b2;
  --paper:#f6f7f9; --card:#ffffff; --line:#e4e7ec;
  --accent:#0f6f5c; --accent2:#0a4c40; --accent-soft:#e7f2ee;
  --warn:#8a5a00; --warn-soft:#fdf3df;
}
*{box-sizing:border-box;}
html{-webkit-print-color-adjust:exact; print-color-adjust:exact; overflow-x:hidden;}
body{
  margin:0; color:var(--ink); background:var(--paper);
  font-family:-apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  font-size:10.5pt; line-height:1.62; overflow-x:hidden;}
.page{max-width:760px; margin:0 auto; padding:46px 40px 40px; overflow-x:hidden;}
.mono{font-family:"SF Mono",ui-monospace,"Cascadia Code",Menlo,Consolas,monospace;
  font-size:0.92em; background:var(--accent-soft); color:var(--accent2);
  padding:1px 5px; border-radius:5px; white-space:normal; word-break:break-word;}
pre.code{background:#0d1117; color:#d7e0ea; border-radius:9px; padding:12px 16px;
  font-family:"SF Mono",ui-monospace,Menlo,Consolas,monospace; font-size:9.3pt;
  line-height:1.55; margin:4px 0 16px; white-space:pre-wrap; overflow-x:hidden;}
p.q{font-weight:700; color:var(--ink); margin:20px 0 4px; font-size:10.6pt;}

/* cover */
.cover{min-height:78vh; display:flex; flex-direction:column; justify-content:center;
  border-bottom:1px solid var(--line); page-break-after:always; margin-bottom:8px;}
.cover .brand{font-size:13pt; letter-spacing:.34em; text-transform:uppercase;
  color:var(--accent); font-weight:700; margin-bottom:22px;}
.cover h1{font-size:40pt; line-height:1.05; margin:0 0 10px; font-weight:800;
  letter-spacing:-.02em; color:var(--ink);}
.cover .tag{font-size:13pt; color:var(--ink2); max-width:52ch; line-height:1.55; margin:0 0 40px;}
.cover .meta{display:flex; gap:34px; flex-wrap:wrap;}
.cover .meta div{font-size:9.5pt;}
.cover .meta .k{color:var(--faint); text-transform:uppercase; letter-spacing:.12em;
  font-size:8pt; margin-bottom:3px;}
.cover .meta .v{color:var(--ink); font-weight:600;}

/* TOC */
.toc{page-break-after:always; margin-bottom:10px;}
.toc h2{font-size:11pt; text-transform:uppercase; letter-spacing:.22em; color:var(--faint);
  margin:0 0 20px;}
.toc ol{list-style:none; counter-reset:s; margin:0; padding:0;}
.toc li{counter-increment:s; display:flex; gap:12px; align-items:baseline;
  padding:7px 0; border-bottom:1px solid var(--line); font-size:10pt;}
.toc li::before{content:counter(s,decimal-leading-zero); color:var(--accent);
  font-weight:700; font-variant-numeric:tabular-nums; min-width:26px;}
.toc li .t{color:var(--ink2); font-weight:500;}

/* headings */
h2{font-size:19pt; font-weight:800; letter-spacing:-.015em; margin:44px 0 6px;
  padding-bottom:10px; border-bottom:2px solid var(--accent); page-break-after:avoid;}
h2 .num{color:var(--accent); font-size:11pt; letter-spacing:.2em; display:block;
  text-transform:uppercase; margin-bottom:4px; font-weight:700;}
h3{font-size:12.5pt; font-weight:700; color:var(--ink); margin:26px 0 8px; page-break-after:avoid;}
p{margin:0 0 12px;}
.lead{font-size:11.5pt; color:var(--ink2); line-height:1.7; margin-bottom:14px;}
ul,ol{margin:0 0 14px; padding-left:22px;}
li{margin:0 0 7px;}
li::marker{color:var(--accent);}
a{color:var(--accent); text-decoration:none;}
hr{border:none; border-top:1px solid var(--line); margin:30px 0;}

/* callouts */
.callout{background:var(--card); border:1px solid var(--line); border-left:4px solid var(--accent);
  border-radius:9px; padding:14px 18px; margin:0 0 18px; page-break-inside:avoid;}
.callout.tip{border-left-color:var(--accent); background:var(--accent-soft);}
.callout .label{display:block; font-size:8.5pt; text-transform:uppercase;
  letter-spacing:.14em; font-weight:700; margin-bottom:5px;}
.callout.tip .label{color:var(--accent2);}
.callout.warn{border-left-color:#e0a020; background:var(--warn-soft);}
.callout.warn .label{color:var(--warn);}
.callout p{margin:0;}

/* tables */
table{width:100%; border-collapse:collapse; margin:6px 0 20px; font-size:9.8pt;
  page-break-inside:auto;}
thead th{background:var(--ink); color:#fff; text-align:left; font-weight:600;
  font-size:8.6pt; text-transform:uppercase; letter-spacing:.08em; padding:9px 13px;
  border-bottom:2px solid var(--accent);}
tbody td{padding:9px 13px; border-bottom:1px solid var(--line); vertical-align:top; color:var(--ink2);}
tbody tr:nth-child(even){background:#fbfcfd;}
tbody tr:last-child td{border-bottom:1px solid var(--line);}
td .k{font-weight:600; color:var(--ink);}
tbody td:first-child{color:var(--ink); font-weight:600;}

/* screenshots */
.figure{background:var(--card); border:1px solid var(--line); border-radius:12px;
  padding:16px; margin:6px 0 22px; page-break-inside:avoid; box-shadow:0 1px 3px rgba(20,25,35,.05);}
.figure .shot{display:flex; justify-content:center; background:#0d1117; border-radius:8px;
  padding:14px; margin-bottom:12px;}
.figure img{width:280px; height:auto; image-rendering:pixelated; border-radius:4px;
  box-shadow:0 2px 10px rgba(0,0,0,.45);}
.figure .cap{text-align:center; font-size:9pt; color:var(--muted); font-style:italic; margin:0;}
.figure .cap b{color:var(--ink2); font-style:normal; font-weight:600;}

/* footer */
.foot{margin-top:40px; padding-top:16px; border-top:1px solid var(--line);
  font-size:8.5pt; color:var(--faint); display:flex; justify-content:space-between; flex-wrap:wrap; gap:8px;}
@media print{
  .page{padding:0; max-width:none;}
  body{background:#fff;}
  .figure{box-shadow:none;}
}
"""


def md_table(rows):
    def cells(r):
        return [c.strip() for c in r.strip().strip("|").split("|")]
    head = cells(rows[0])
    body = [cells(r) for r in rows[2:] if r.count("|") >= 1]
    out = ['<table><thead><tr>']
    out += [f"<th>{esc(h)}</th>" for h in head]
    out.append("</tr></thead><tbody>")
    for r in body:
        out.append("<tr>")
        for i, c in enumerate(r):
            cls = ' class="k"' if i == 0 else ""
            out.append(f"<td{cls}>{inline(c)}</td>")
        out.append("</tr>")
    out.append("</tbody></table>")
    return "".join(out)


def esc(s):
    return html.escape(s)


def inline(s):
    s = esc(s)
    s = re.sub(r"`([^`]+)`", r'<code class="mono">\1</code>', s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", s)
    return s


def render(md):
    lines = md.split("\n")
    out = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        # fenced code block
        if line.strip().startswith("```"):
            buf = []
            i += 1
            while i < n and not lines[i].strip().startswith("```"):
                buf.append(lines[i]); i += 1
            i += 1  # skip closing fence
            out.append('<pre class="code">' + esc("\n".join(buf)) + "</pre>")
            continue
        if line.startswith("|") and i + 1 < n and lines[i + 1].strip().startswith("|") \
           and set(lines[i + 1].strip()) <= set("|- :"):
            block = []
            while i < n and lines[i].strip().startswith("|"):
                block.append(lines[i]); i += 1
            out.append(md_table(block)); continue
        if line.startswith("### "):
            out.append(f"<h3>{inline(line[4:])}</h3>"); i += 1; continue
        if line.startswith("## "):
            m = re.match(r"## (\d+)\.\s+(.*)", line)
            if m:
                num, title = m.group(1), inline(m.group(2))
                out.append(f'<h2><span class="num">Section {num}</span>{title}</h2>')
            else:
                out.append(f'<h2>{inline(line[3:])}</h2>')
            i += 1; continue
        # standalone bold line = troubleshooting question
        if re.match(r"^\*\*.+\*\*$", line.strip()):
            out.append(f'<p class="q">{inline(line.strip())}</p>'); i += 1; continue
        if line.startswith("- "):
            items = []
            while i < n and lines[i].startswith("- "):
                items.append(inline(lines[i][2:])); i += 1
            out.append("<ul>" + "".join(f"<li>{x}</li>" for x in items) + "</ul>"); continue
        if re.match(r"^\d+\.\s", line):
            items = []
            while i < n and re.match(r"^\d+\.\s", lines[i]):
                items.append(inline(re.sub(r"^\d+\.\s", "", lines[i]))); i += 1
            out.append("<ol>" + "".join(f"<li>{x}</li>" for x in items) + "</ol>"); continue
        if line.startswith("> "):
            # gather the full multi-line blockquote into ONE callout
            quote = []
            while i < n and lines[i].startswith(">"):
                q = lines[i]
                q = q[1:] if q.startswith("> ") else q[2:]
                quote.append(q.strip())
                i += 1
            # drop any blank separators
            quote = [q for q in quote if q != ""]
            label = ""
            joined = " ".join(quote)
            if "💡" in joined: label = "Tip"
            elif "⚠" in joined: label = "Good to know"
            txt = inline(" ".join(quote))
            cls = "tip" if "💡" in joined else "warn"
            lab = f'<span class="label">{label}</span>' if label else ""
            out.append(f'<div class="callout {cls}">{lab}<p>{txt}</p></div>')
            continue
        if line.strip() == "---":
            out.append("<hr>"); i += 1; continue
        if line.strip():
            para = [line]
            i += 1
            while i < n and lines[i].strip() and not re.match(
                    r"^(#{1,3} |> |[-*] |\d+\. |\||---|```)", lines[i]):
                para.append(lines[i]); i += 1
            out.append(f"<p>{inline(' '.join(p.strip() for p in para))}</p>")
            continue
        i += 1
    return "\n".join(out)


def main():
    md = open(os.path.join(HERE, "..", "USER_MANUAL.md"), encoding="utf-8").read()

    # strip the H1 title block and the embedded TOC (we render our own cover + TOC)
    body_md = md.split("## 1.", 1)[1]
    body_md = "## 1." + body_md
    # drop the original numbered TOC list (between '## Table of Contents' and '## 1.') — already removed by split
    # also drop any leftover '1. [..](#..)' TOC lines that slipped before section 1
    body_md = re.sub(r"(?m)^\d+\.\s+\[[^\]]+\]\([^)]*\)\s*$", "", body_md)

    toc = []
    for m in re.finditer(r"^## (\d+)\.\s+(.+)$", body_md, re.M):
        toc.append((m.group(1), m.group(2)))

    # (figures are injected after rendering, against the rendered <h2> HTML)

    body = render(body_md)

    # ---- inject figure blocks after specific rendered headings ----
    fig_intro = (
        '<div class="figure"><div class="shot"><img src="assets/frame_300.png" '
        'alt="Game Boy Advance boot screen"></div>'
        '<p class="cap"><b>CrabBoy in action.</b> The moment a game boots, you'
        '&rsquo;ll see real, fully-rendered game screens just like this.</p></div>')
    body = body.replace('<span class="num">Section 3</span>Your First Game',
                        '<span class="num">Section 3</span>Your First Game'
                        + '</h2>' + fig_intro + '<h2>', 1)

    fig_rewind = (
        '<div class="figure"><div class="shot"><img src="assets/frame_900.png" '
        'alt="In-game scene"></div>'
        '<p class="cap"><b>Walked into trouble?</b> Live Rewind lets you step back '
        'through the last few seconds and try the same moment again &mdash; no '
        'reloading, no waiting.</p></div>')
    body = body.replace('<span class="num">Section 6</span>Undo Mistakes with Live Rewind',
                        '<span class="num">Section 6</span>Undo Mistakes with Live Rewind'
                        + '</h2>' + fig_rewind + '<h2>', 1)

    fig_companion = (
        '<div class="figure"><div class="shot"><img src="assets/frame_600.png" '
        'alt="A forest scene in game"></div>'
        '<p class="cap"><b>Exploring the open world.</b> Scenes like this are '
        'where the Pok&eacute;mon Companion, the AI Agent Player, and your own '
        'creativity all come together.</p></div>')
    body = body.replace('<span class="num">Section 11</span>Keeping Up to Date',
                        '<span class="num">Section 11</span>Keeping Up to Date'
                        + '</h2>' + fig_companion + '<h2>', 1)

    fig_close = (
        '<div class="figure"><div class="shot"><img src="assets/frame_1200.png" '
        'alt="A forest path in game"></div>'
        '<p class="cap"><b>Your adventure, on your terms.</b> CrabBoy gives you the '
        'tools &mdash; the journey is yours to take.</p></div>')
    body = body.replace("<p>Happy playing. 🦀</p>",
                        fig_close + "<p>Happy playing. 🦀</p>", 1)

    # report which injections landed
    for name, needle in [("frame_300","Your First Game"),("frame_900","Undo Mistakes"),
                         ("frame_600","Keeping Up to Date"),("frame_1200","Happy playing")]:
        print(name, "embedded" if needle in body else "MISSING ANCHOR")

    toc_html = '<ol>' + "".join(
        f'<li><span class="t">{esc(t)}</span></li>' for _, t in toc) + "</ol>"

    html_doc = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>The Player&rsquo;s Manual &mdash; CrabBoy Advance</title>
<style>{CSS}</style></head><body>
<div class="page">
  <div class="cover">
    <div class="brand">CrabBoy Advance</div>
    <h1>The Player&rsquo;s<br>Manual</h1>
    <p class="tag">Everything you need to play your Game Boy Advance games on your computer
    &mdash; written in plain, friendly language, with no technical background required.</p>
    <div class="meta">
      <div><div class="k">Edition</div><div class="v">v0.5.0</div></div>
      <div><div class="k">Release</div><div class="v">August 2026</div></div>
      <div><div class="k">Platform</div><div class="v">Linux &middot; Windows</div></div>
      <div><div class="k">Audience</div><div class="v">Everyone</div></div>
    </div>
  </div>

  <div class="toc">
    <h2>In this manual</h2>
    {toc_html}
  </div>

  {body}

  <div class="foot">
    <span>The Player&rsquo;s Manual &middot; CrabBoy Advance v0.5.0</span>
    <span>Free &amp; open source &middot; Built with care</span>
  </div>
</div>
</body></html>"""

    outp = os.path.join(HERE, "manual.html")
    open(outp, "w", encoding="utf-8").write(html_doc)
    print("wrote", outp, len(html_doc), "bytes;", len(toc), "toc entries")
    for f in ("frame_300", "frame_600", "frame_900", "frame_1200"):
        print(f, "embedded" if f in html_doc else "MISSING")


if __name__ == "__main__":
    main()
