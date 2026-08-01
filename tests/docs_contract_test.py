#!/usr/bin/env python3
"""Repository-local documentation lifecycle and link contract.

The checker is deliberately dependency-free and offline. External URLs remain
the responsibility of their owners; every relative Markdown path and fragment
must resolve in the checked-out tree so documentation moves cannot silently
leave stale navigation behind.
"""

from __future__ import annotations

import pathlib
import re
import unittest
import urllib.parse


ROOT = pathlib.Path(__file__).resolve().parents[1]
DESIGN = ROOT / "docs" / "design"
LINK = re.compile(r"(?<!!)\[[^\]]+\]\((<[^>]+>|[^)\s]+)(?:\s+['\"][^)]*['\"])?\)")
FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$")
LIFECYCLE = re.compile(
    r"^> \*\*Lifecycle: (Active|Implemented|Superseded|Historical|Validation evidence)\.\*\*",
    re.MULTILINE,
)


def markdown_files() -> list[pathlib.Path]:
    files = []
    for path in ROOT.rglob("*.md"):
        relative = path.relative_to(ROOT)
        if relative.parts[0] in {".git", "target", "test-results"}:
            continue
        files.append(path)
    return sorted(files)


def prose(text: str) -> str:
    output = []
    fence = None
    for line in text.splitlines():
        marker = FENCE.match(line)
        if marker:
            current = marker.group(1)[0]
            if fence is None:
                fence = current
            elif fence == current:
                fence = None
            continue
        if fence is None:
            output.append(line)
    return "\n".join(output)


def heading_anchors(text: str) -> set[str]:
    anchors = set()
    occurrences: dict[str, int] = {}
    for line in prose(text).splitlines():
        match = HEADING.match(line)
        if not match:
            continue
        label = re.sub(r"<[^>]+>", "", match.group(1)).strip().lower()
        label = re.sub(r"[^\w\- ]", "", label, flags=re.UNICODE)
        # GitHub removes punctuation first and then replaces each remaining
        # whitespace character with `-`; it does not collapse the resulting
        # adjacent hyphens (for example text around ` = ` becomes `--`).
        base = re.sub(r"\s", "-", label).strip("-")
        index = occurrences.get(base, 0)
        occurrences[base] = index + 1
        anchors.add(base if index == 0 else f"{base}-{index}")
    return anchors


def relative_links(path: pathlib.Path) -> list[tuple[str, pathlib.Path, str]]:
    links = []
    for match in LINK.finditer(prose(path.read_text(encoding="utf-8"))):
        raw = match.group(1).strip("<>")
        parsed = urllib.parse.urlsplit(raw)
        if parsed.scheme or raw.startswith("//"):
            continue
        target_text = urllib.parse.unquote(parsed.path)
        target = path if not target_text else (path.parent / target_text).resolve()
        links.append((raw, target, urllib.parse.unquote(parsed.fragment)))
    return links


class DocumentationContractTests(unittest.TestCase):
    def test_every_design_document_has_a_lifecycle_and_index_entry(self) -> None:
        index = (DESIGN / "README.md").read_text(encoding="utf-8")
        for document in sorted(DESIGN.glob("*.md")):
            head = "\n".join(document.read_text(encoding="utf-8").splitlines()[:30])
            self.assertTrue(
                LIFECYCLE.search(head),
                f"{document.relative_to(ROOT)} has no recognized lifecycle",
            )
            self.assertIn(
                f"[{document.name}]({document.name})",
                index,
                f"{document.relative_to(ROOT)} is missing from the design index",
            )

    def test_every_relative_markdown_path_and_anchor_resolves(self) -> None:
        failures = []
        anchor_cache: dict[pathlib.Path, set[str]] = {}
        for source in markdown_files():
            for raw, target, fragment in relative_links(source):
                if not target.exists():
                    failures.append(
                        f"{source.relative_to(ROOT)} -> {raw}: target does not exist"
                    )
                    continue
                if not fragment or target.is_dir() or target.suffix.lower() != ".md":
                    continue
                anchors = anchor_cache.setdefault(
                    target,
                    heading_anchors(target.read_text(encoding="utf-8")),
                )
                if fragment.lower() not in anchors:
                    failures.append(
                        f"{source.relative_to(ROOT)} -> {raw}: anchor not found in "
                        f"{target.relative_to(ROOT)}"
                    )
        self.assertEqual(failures, [], "\n" + "\n".join(failures))


if __name__ == "__main__":
    unittest.main()
