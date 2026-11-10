#!/usr/bin/env python3
"""Generate packages.tsv: what defines each command mathfmt emits, and whether
unicode-math draws it as the character it spells.

For every control word in a spelling of latex.tsv or greek.tsv, and every
math-alphabet command of signatures.tsv, pdflatex is asked where it is
defined: in the LaTeX kernel, or after loading exactly one of the packages
below (the first in this order that defines it wins), or nowhere (`-`). A
kernel stub that only says "not provided in base LaTeX2e" (`\\not@base`: mho,
leadsto) does not count as defined.

lualatex is then asked whether unicode-math defines the command (`yes`/`no`),
since that package replaces amssymb and friends and lacks some of their
names, and, for a command that spells one character, whether it typesets the
same glyphs there as the character does (`same`/`differs`): a few of
amssymb's names point at another glyph under unicode-math
(`\\blacktriangleright` is the large triangle, `\\backepsilon` the lunate
epsilon). The glyphs are read off the typeset boxes with Lua.

The answers are what `mathfmt declarations` uses to name the packages a
preamble lacks, and what the unicode-math preset uses to pick a spelling that
exists and draws the atom's character.

It also writes defined.tsv: every user-level name (letters only) the kernel
defines, and every one each package adds, as LuaTeX's hash lists them after
loading it. That is how a `\\providecommand` of a name a package already
defines (`\\implies` under amsmath) is known to have done nothing.

Writes crates/mathfmt-core/data/packages.tsv and defined.tsv. Needs pdflatex
and lualatex with the packages installed. Run by hand when a table gains a command or TeX
Live updates; review the diff.

    tools/gen-packages.py
"""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

DATA = Path(__file__).resolve().parent.parent / "crates/mathfmt-core/data"

# (name in the table, engine, preamble line). Order is preference: a command
# several packages define is credited to the first.
SOURCES = [
    ("kernel", "pdflatex", ""),
    ("amsmath", "pdflatex", r"\usepackage{amsmath}"),
    ("amsfonts", "pdflatex", r"\usepackage{amsfonts}"),
    ("amssymb", "pdflatex", r"\usepackage{amssymb}"),
    ("bm", "pdflatex", r"\usepackage{bm}"),
    ("bbm", "pdflatex", r"\usepackage{bbm}"),
    ("mathrsfs", "pdflatex", r"\usepackage{mathrsfs}"),
    ("dsfont", "pdflatex", r"\usepackage{dsfont}"),
]
UNICODE_MATH = ("lualatex", r"\usepackage{unicode-math}")

WORD = re.compile(r"\\([A-Za-z]+)")


def rows(name):
    for line in (DATA / name).read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        yield line.split("\t")


def candidates():
    names = set()
    for row in rows("latex.tsv"):
        if len(row) > 1:
            names.update(WORD.findall(row[1]))
    for row in rows("greek.tsv"):
        names.update(WORD.findall(row[1]))
    for row in rows("signatures.tsv"):
        if len(row) > 2 and ("style=" in row[2] or "plain" in row[2].split()):
            names.add(row[0])
    return sorted(names)


def spelled_characters():
    """command -> codepoint, for every spelling that is one command."""
    pairs = {}
    for name in ("latex.tsv", "greek.tsv"):
        for row in rows(name):
            if len(row) < 2:
                continue
            spelling = row[1].strip()
            if re.fullmatch(r"\\[A-Za-z]+", spelling):
                pairs.setdefault(spelling[1:], row[0].strip().upper())
    return pairs


def run_tex(engine, work, job):
    # Long lines, so the log does not wrap a name or a meaning.
    env = dict(os.environ, max_print_line="100000")
    out = subprocess.run(
        [engine, "-interaction=nonstopmode", "-halt-on-error", job],
        cwd=work,
        capture_output=True,
        text=True,
        errors="replace",
        env=env,
    )
    if out.returncode != 0:
        sys.exit(f"{engine} failed on {job}:\n{out.stdout[-2000:]}")
    return out.stdout


def defined(engine, preamble, names, work):
    tex = [r"\documentclass{article}", preamble, r"\begin{document}"]
    # A command is defined when its control sequence exists and is not
    # \relax; its meaning, and that of the inner command a robust one
    # wraps, is printed so a kernel stub can be told apart.
    tex.append(
        r"\def\probe#1{\ifcsname #1\endcsname"
        r"\expandafter\ifx\csname #1\endcsname\relax\else"
        r"\typeout{DEFINED:#1:\expandafter\meaning\csname #1\endcsname"
        r"|\expandafter\meaning\csname #1 \endcsname:END}\fi\fi}"
    )
    tex += [rf"\probe{{{n}}}" for n in names]
    tex.append(r"\end{document}")
    (work / "probe.tex").write_text("\n".join(tex) + "\n", encoding="utf-8")
    out = run_tex(engine, work, "probe.tex")
    found = set()
    for name, meaning in re.findall(r"DEFINED:([A-Za-z]+):(.*?):END", out, re.S):
        # LaTeX's stub for an amssymb name (`\mho`, `\leadsto`) is
        # `\not@base \mho`, an error when used.
        if "not provided in base LaTeX2e" in meaning or "\\not@base" in meaning:
            continue
        found.add(name)
    return found


def glyphs_match(pairs, work):
    """Under lualatex with unicode-math: does `\\name` typeset the same glyphs
    as its character? The glyph nodes of both boxes, read with Lua."""
    tex = [r"\documentclass{article}", UNICODE_MATH[1], r"\begin{document}"]
    tex.append(
        r"\directlua{"
        r"function mathfmt_glyphs(n) local out = {} "
        r"local function walk(h) for v in node.traverse(h) do "
        r"if v.id == node.id('glyph') then out[\string#out+1] = tostring(v.char) "
        r"elseif v.list then walk(v.list) end end end "
        r"walk(tex.box[n].list) return table.concat(out, '+') end}"
    )
    tex.append(
        r"\def\probe#1#2{\setbox0\hbox{$\csname #1\endcsname$}"
        r'\setbox2\hbox{$\Uchar"#2$}'
        r"\typeout{GLYPHS:#1:\directlua{tex.print(mathfmt_glyphs(0))}:"
        r"\directlua{tex.print(mathfmt_glyphs(2))}:END}}"
    )
    for name, cp in sorted(pairs.items()):
        tex.append(rf"\probe{{{name}}}{{{cp}}}")
    tex.append(r"\end{document}")
    (work / "glyphs.tex").write_text("\n".join(tex) + "\n", encoding="utf-8")
    out = run_tex("lualatex", work, "glyphs.tex")
    same = {}
    for name, a, b in re.findall(r"GLYPHS:([A-Za-z]+):([^:]*):([^:]*):END", out):
        # Both boxes empty: the font has neither, and nothing differs.
        same[name] = "same" if a == b else "differs"
        if same[name] == "differs":
            hex_ = lambda s: "+".join(f"U+{int(x):04X}" for x in s.split("+") if x)
            print(f"  \\{name} draws {hex_(a) or 'nothing'}, the character {hex_(b)}")
    return same


# The packages whose names defined.tsv lists, after the kernel's.
DEFINED_BY = ["amsmath", "amsfonts", "amssymb", "amsthm", "mathtools", "bm", "bbm", "mathrsfs", "dsfont"]


def hash_names(preamble, work):
    """Every letters-only defined name in LuaTeX's hash after `preamble`."""
    tex = [
        r"\documentclass{article}",
        preamble,
        r"\begin{document}",
        r"\directlua{local f = io.open('names.txt', 'w') "
        r"for _, n in ipairs(tex.hashtokens()) do "
        r"if n:match('^[A-Za-z]+$') and token.is_defined(n) then f:write(n, '\string\n') end end "
        r"f:close()}",
        r"\end{document}",
    ]
    (work / "hash.tex").write_text("\n".join(tex) + "\n", encoding="utf-8")
    run_tex("lualatex", work, "hash.tex")
    return set((work / "names.txt").read_text(encoding="utf-8").split())


def write_defined(work):
    kernel = hash_names("", work)
    lines = [
        "# name\tdefined by",
        "# generated by tools/gen-packages.py; do not edit. Every letters-only name",
        "# LuaTeX's hash holds after the kernel (`kernel`) or after one package that",
        "# adds it, listed by the first package that does.",
    ]
    rows = {n: "kernel" for n in kernel}
    for package in DEFINED_BY:
        for n in hash_names(rf"\usepackage{{{package}}}", work) - kernel:
            rows.setdefault(n, package)
    lines += [f"{n}\t{rows[n]}" for n in sorted(rows)]
    (DATA / "defined.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"defined.tsv: {len(rows)} names, {len(kernel)} of them the kernel's")


def main():
    names = candidates()
    where = {}
    with tempfile.TemporaryDirectory() as tmp:
        for source, engine, preamble in SOURCES:
            for n in defined(engine, preamble, names, Path(tmp)):
                where.setdefault(n, source)
        um = defined(UNICODE_MATH[0], UNICODE_MATH[1], names, Path(tmp))
        spelled = {n: cp for n, cp in spelled_characters().items() if n in um}
        same = glyphs_match(spelled, Path(tmp))
        write_defined(Path(tmp))
    lines = [
        "# command\tdefined by (pdflatex)\tunicode-math\tunicode-math glyph",
        "# generated by tools/gen-packages.py; do not edit. `kernel` needs no package;",
        "# `-` is defined by none of the packages the script tries under pdflatex.",
        "# The third column says whether lualatex with unicode-math defines it; the",
        "# fourth whether, there, it draws the same glyphs as the character it spells",
        "# (`-`: not a character's spelling, or not defined there).",
    ]
    missing = []
    for n in names:
        glyph = same.get(n, "-")
        lines.append(f"{n}\t{where.get(n, '-')}\t{'yes' if n in um else 'no'}\t{glyph}")
        if n not in where:
            missing.append(n)
    (DATA / "packages.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")
    not_um = [n for n in names if n not in um]
    other = [n for n in names if same.get(n) == "differs"]
    print(f"{len(names)} commands; {len(missing)} defined by no pdflatex package: {' '.join(missing)}")
    print(f"{len(not_um)} not defined by unicode-math: {' '.join(not_um)}")
    print(f"{len(other)} draw another glyph than their character under unicode-math: {' '.join(other)}")


if __name__ == "__main__":
    main()
