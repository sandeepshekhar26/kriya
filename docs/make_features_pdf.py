#!/usr/bin/env python3
"""Render FEATURES.md to a styled PDF, reusing the converter + CSS from make_pdf.py."""

import os
import subprocess

from make_pdf import md_to_html, CSS  # same renderer/styling as the explainer PDF

HERE = os.path.dirname(os.path.abspath(__file__))
MD_PATH = os.path.join(HERE, "FEATURES.md")
HTML_PATH = os.path.join(HERE, "FEATURES_tmp.html")
PDF_PATH = os.path.join(HERE, "FEATURES.pdf")


def main():
    with open(MD_PATH, "r", encoding="utf-8") as f:
        md_text = f.read()

    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Kriya — Feature List</title>
<style>
{CSS}
</style>
</head>
<body>
{md_to_html(md_text)}
</body>
</html>"""

    with open(HTML_PATH, "w", encoding="utf-8") as f:
        f.write(html)
    print(f"✓ HTML written to {HTML_PATH}")

    result = subprocess.run(
        ["/usr/sbin/cupsfilter", "-o", "media=Letter", HTML_PATH, "-o", PDF_PATH],
        capture_output=True, text=True,
    )
    if result.returncode == 0 and os.path.exists(PDF_PATH):
        print(f"✓ PDF created at {PDF_PATH}")
        return

    print("cupsfilter failed:")
    print(result.stderr[:500])


if __name__ == "__main__":
    main()
