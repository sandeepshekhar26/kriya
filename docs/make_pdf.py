#!/usr/bin/env python3
"""Convert EXPLAINER.md to a styled PDF using a pure-Python markdown renderer + osascript."""

import re
import os
import subprocess
import sys
import tempfile

MD_PATH = os.path.join(os.path.dirname(__file__), "EXPLAINER.md")
HTML_PATH = os.path.join(os.path.dirname(__file__), "EXPLAINER_tmp.html")
PDF_PATH = os.path.join(os.path.dirname(__file__), "EXPLAINER.pdf")

# ── Minimal markdown → HTML converter ──────────────────────────────────────

def md_to_html(text: str) -> str:
    lines = text.split("\n")
    html_lines = []
    in_code = False
    in_ul = False
    in_ol = False
    in_blockquote = False
    in_table = False
    table_rows = []

    def close_lists():
        nonlocal in_ul, in_ol
        if in_ul:
            html_lines.append("</ul>")
            in_ul = False
        if in_ol:
            html_lines.append("</ol>")
            in_ol = False

    def close_blockquote():
        nonlocal in_blockquote
        if in_blockquote:
            html_lines.append("</blockquote>")
            in_blockquote = False

    def flush_table():
        nonlocal in_table, table_rows
        if not in_table or not table_rows:
            return
        html_lines.append('<table>')
        for i, row in enumerate(table_rows):
            cols = [c.strip() for c in row.strip("|").split("|")]
            tag = "th" if i == 0 else "td"
            html_lines.append("<tr>" + "".join(f"<{tag}>{inline(c)}</{tag}>" for c in cols) + "</tr>")
        html_lines.append("</table>")
        in_table = False
        table_rows = []

    def inline(s: str) -> str:
        # code spans
        s = re.sub(r'`([^`]+)`', r'<code>\1</code>', s)
        # bold+italic
        s = re.sub(r'\*\*\*(.+?)\*\*\*', r'<strong><em>\1</em></strong>', s)
        # bold
        s = re.sub(r'\*\*(.+?)\*\*', r'<strong>\1</strong>', s)
        # italic
        s = re.sub(r'\*(.+?)\*', r'<em>\1</em>', s)
        # links
        s = re.sub(r'\[([^\]]+)\]\(([^)]+)\)', r'<a href="\2">\1</a>', s)
        return s

    i = 0
    while i < len(lines):
        line = lines[i]

        # fenced code block
        if line.startswith("```"):
            flush_table()
            close_lists()
            close_blockquote()
            if not in_code:
                lang = line[3:].strip()
                html_lines.append(f'<pre><code class="language-{lang}">')
                in_code = True
            else:
                html_lines.append("</code></pre>")
                in_code = False
            i += 1
            continue

        if in_code:
            html_lines.append(line.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))
            i += 1
            continue

        # table (pipe separated)
        if "|" in line and line.strip().startswith("|"):
            if not in_table:
                flush_table()
                close_lists()
                close_blockquote()
                in_table = True
            # skip separator rows (e.g. |---|---|)
            if re.match(r'^\|[\s\-|:]+\|$', line.strip()):
                i += 1
                continue
            table_rows.append(line)
            i += 1
            continue
        elif in_table:
            flush_table()

        # HR
        if re.match(r'^---+$', line.strip()):
            close_lists()
            close_blockquote()
            html_lines.append("<hr>")
            i += 1
            continue

        # headings
        m = re.match(r'^(#{1,6})\s+(.*)', line)
        if m:
            flush_table()
            close_lists()
            close_blockquote()
            level = len(m.group(1))
            html_lines.append(f"<h{level}>{inline(m.group(2))}</h{level}>")
            i += 1
            continue

        # blockquote
        if line.startswith("> "):
            flush_table()
            close_lists()
            if not in_blockquote:
                html_lines.append("<blockquote>")
                in_blockquote = True
            html_lines.append(f"<p>{inline(line[2:])}</p>")
            i += 1
            continue
        elif in_blockquote and line.strip() == "":
            close_blockquote()

        # ordered list
        m = re.match(r'^(\d+)\.\s+(.*)', line)
        if m:
            flush_table()
            close_blockquote()
            if in_ul:
                html_lines.append("</ul>")
                in_ul = False
            if not in_ol:
                html_lines.append("<ol>")
                in_ol = True
            html_lines.append(f"<li>{inline(m.group(2))}</li>")
            i += 1
            continue

        # unordered list
        m = re.match(r'^[-*]\s+(.*)', line)
        if m:
            flush_table()
            close_blockquote()
            if in_ol:
                html_lines.append("</ol>")
                in_ol = False
            if not in_ul:
                html_lines.append("<ul>")
                in_ul = True
            html_lines.append(f"<li>{inline(m.group(1))}</li>")
            i += 1
            continue

        # blank line
        if line.strip() == "":
            close_lists()
            close_blockquote()
            flush_table()
            i += 1
            continue

        # paragraph
        close_lists()
        close_blockquote()
        flush_table()
        html_lines.append(f"<p>{inline(line)}</p>")
        i += 1

    close_lists()
    close_blockquote()
    flush_table()
    return "\n".join(html_lines)


CSS = """
@import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap');

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;
  font-size: 11pt;
  line-height: 1.7;
  color: #1a1a2e;
  background: #ffffff;
  max-width: 800px;
  margin: 0 auto;
  padding: 48px 56px;
}

h1 {
  font-size: 26pt;
  font-weight: 700;
  color: #0f0c29;
  margin-bottom: 6px;
  letter-spacing: -0.5px;
  border-bottom: 3px solid #6c63ff;
  padding-bottom: 10px;
}

h2 {
  font-size: 15pt;
  font-weight: 700;
  color: #302b63;
  margin-top: 30px;
  margin-bottom: 12px;
  padding-left: 10px;
  border-left: 4px solid #6c63ff;
}

h3 {
  font-size: 12pt;
  font-weight: 600;
  color: #302b63;
  margin-top: 20px;
  margin-bottom: 8px;
}

p {
  margin-bottom: 12px;
  color: #2d2d44;
}

strong { color: #1a1a2e; font-weight: 600; }
em { color: #4a4a6a; }

ul, ol {
  margin: 8px 0 14px 24px;
  color: #2d2d44;
}

li { margin-bottom: 5px; }

blockquote {
  border-left: 4px solid #6c63ff;
  background: linear-gradient(135deg, #f0eeff 0%, #e8f4ff 100%);
  margin: 18px 0;
  padding: 14px 20px;
  border-radius: 0 8px 8px 0;
}

blockquote p {
  margin: 0;
  font-style: italic;
  color: #302b63;
  font-size: 11.5pt;
}

pre {
  background: #0f0c29;
  color: #c9d1d9;
  border-radius: 8px;
  padding: 16px 20px;
  margin: 14px 0;
  overflow-x: auto;
  font-size: 9pt;
  line-height: 1.5;
}

code {
  font-family: 'JetBrains Mono', 'SF Mono', Consolas, monospace;
}

p code, li code {
  background: #eeeeff;
  color: #5b50d6;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 9.5pt;
}

table {
  width: 100%;
  border-collapse: collapse;
  margin: 16px 0;
  font-size: 10pt;
  border-radius: 8px;
  overflow: hidden;
  box-shadow: 0 0 0 1px #e0deff;
}

th {
  background: linear-gradient(135deg, #6c63ff 0%, #302b63 100%);
  color: #ffffff;
  padding: 10px 14px;
  text-align: left;
  font-weight: 600;
  font-size: 10pt;
}

td {
  padding: 9px 14px;
  border-bottom: 1px solid #e8e8ff;
  color: #2d2d44;
  vertical-align: top;
}

tr:nth-child(even) td { background: #f7f7ff; }
tr:last-child td { border-bottom: none; }

hr {
  border: none;
  border-top: 1.5px solid #e0deff;
  margin: 28px 0;
}

a { color: #6c63ff; text-decoration: none; }

@media print {
  body { padding: 20px 32px; }
  h2 { page-break-after: avoid; }
  pre, table, blockquote { page-break-inside: avoid; }
}
"""

def build_html(md_text: str) -> str:
    body = md_to_html(md_text)
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>verb — Plain-English Explainer</title>
<style>
{CSS}
</style>
</head>
<body>
{body}
</body>
</html>"""


def main():
    with open(MD_PATH, "r", encoding="utf-8") as f:
        md_text = f.read()

    html = build_html(md_text)

    with open(HTML_PATH, "w", encoding="utf-8") as f:
        f.write(html)

    print(f"✓ HTML written to {HTML_PATH}")

    # Use macOS osascript to print HTML to PDF via Safari
    abs_html = os.path.abspath(HTML_PATH)
    abs_pdf  = os.path.abspath(PDF_PATH)

    script = f'''
tell application "Safari"
    activate
    open POSIX file "{abs_html}"
    delay 3
    tell application "System Events"
        keystroke "p" using {{command down}}
        delay 2
        -- Switch to PDF
        key code 125 -- down arrow to select PDF
        delay 0.5
    end tell
end tell
'''

    # Better approach: use cupsfilter or webkit2pdf or sips
    # Actually, use the webkit-based htmldoc if available, else use osascript with Pages
    # Most reliable on macOS: use wkhtmltopdf or Chromium headless
    # Let's try: /usr/bin/cupsfilter

    result = subprocess.run(
        ["/usr/sbin/cupsfilter", "-o", "media=Letter", abs_html, "-o", abs_pdf],
        capture_output=True, text=True
    )
    if result.returncode == 0 and os.path.exists(abs_pdf):
        print(f"✓ PDF created at {abs_pdf}")
        return

    print("cupsfilter failed, trying alternative...")
    print(result.stderr[:300])

    # Fallback: use osascript to open in Safari and export PDF
    osa = f'''
set htmlFile to POSIX file "{abs_html}"
set pdfFile to POSIX file "{abs_pdf}"
tell application "Safari"
    activate
    open htmlFile
    delay 4
    do JavaScript "window.print()" in front document
end tell
'''
    subprocess.run(["osascript", "-e", osa])
    print("Safari print dialog opened. Please save as PDF manually if needed.")


if __name__ == "__main__":
    main()
