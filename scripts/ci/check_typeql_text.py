#!/usr/bin/env python3
"""Reject production TypeQL construction outside the native AST adapter."""

from __future__ import annotations

import ast
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ADAPTER = "type-bridge-core/crates/core/src/tql/"
KEYWORDS = r"match|insert|delete|update|put|fetch|define|undefine|redefine|isa|has|links|sort|offset|limit|reduce"
KEYWORD = re.compile(rf"(?<![.#])\b(?:{KEYWORDS})\b")
ANNOTATION = re.compile(
    r"@(?:key|unique|distinct|card|abstract|independent|doc|meta|range|regex|values)\b"
)
TOKEN = re.compile(
    r'(?P<space>\s+)|(?P<raw>(?:br|r)(?P<hash>#+)".*?"(?P=hash)|(?:br|r)"[^"]*")'
    r'|(?P<string>b?"(?:\\.|[^"\\])*")|(?P<char>\'(?:\\.|[^\'\\])\')'
    r"|(?P<word>[A-Za-z_]\w*)|(?P<other>.)",
    re.S,
)


def constructs_typeql(text: str) -> bool:
    """Recognize wire syntax, distinguishing ordinary diagnostic prose."""
    text = re.sub(r"\{[A-Za-z_]\w*(?::[^{}]+)?\}", "{}", text)
    if ANNOTATION.search(text) and (
        re.search(r"@(?:card|doc|meta|range|regex|values)\s*\(", text)
        or re.search(r"@(?:key|unique|distinct|abstract|independent)\s*$", text)
    ):
        return True
    if not KEYWORD.search(text):
        return False
    if text.strip() in KEYWORDS.split("|") or "$" in text:
        return True
    return bool(
        re.search(rf"(?:^|\n)\s*(?:{KEYWORDS})\s*(?:\{{|\d)", text)
        or re.match(r"\s*\{\}\s+(?:isa|has|links)\b", text)
        or re.match(r"\s*(?:define|redefine)\s+(?:entity|relation|attribute|fun|struct)\b", text)
        or re.match(r"\s*(?:has)\b.*\{\}", text)
    )


def python_violations(source: str) -> list[int]:
    tree = ast.parse(source)
    parents = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    found = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.JoinedStr | ast.BinOp):
            continue
        if isinstance(node, ast.BinOp) and not isinstance(node.op, ast.Add):
            continue
        ancestor = node
        diagnostic = False
        while ancestor in parents:
            ancestor = parents[ancestor]
            if isinstance(ancestor, ast.Raise | ast.Assert):
                diagnostic = True
            if isinstance(ancestor, ast.Call):
                name = ancestor.func
                if isinstance(name, ast.Name) and name.id.endswith("Error"):
                    diagnostic = True
                if isinstance(name, ast.Attribute) and isinstance(name.value, ast.Name):
                    diagnostic |= name.value.id in {"logger", "logging", "log"}
        if diagnostic:
            continue
        if isinstance(node, ast.JoinedStr):
            text = "".join(
                value.value if isinstance(value, ast.Constant) else "{}" for value in node.values
            )
        else:
            text = "".join(
                value.value
                for value in ast.walk(node)
                if isinstance(value, ast.Constant) and isinstance(value.value, str)
            )
        if constructs_typeql(text):
            found.add(node.lineno)
    return sorted(found)


@dataclass
class Token:
    text: str
    start: int
    kind: str


def rust_tokens(source: str) -> list[Token]:
    tokens = []
    position = 0
    while position < len(source):
        if source.startswith("//", position):
            end = source.find("\n", position)
            position = len(source) if end < 0 else end
            continue
        if source.startswith("/*", position):
            depth = 1
            position += 2
            while depth and position < len(source):
                if source.startswith("/*", position):
                    depth += 1
                    position += 2
                elif source.startswith("*/", position):
                    depth -= 1
                    position += 2
                else:
                    position += 1
            continue
        match = TOKEN.match(source, position)
        assert match is not None
        kind = match.lastgroup or "other"
        if kind != "space":
            tokens.append(Token(match[0], position, kind))
        position = match.end()
    return tokens


def closing(tokens: list[Token], start: int) -> int:
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = [pairs[tokens[start].text]]
    for index in range(start + 1, len(tokens)):
        text = tokens[index].text
        if text in pairs:
            stack.append(pairs[text])
        elif stack and text == stack[-1]:
            stack.pop()
            if not stack:
                return index
    return len(tokens) - 1


def literal(token: Token) -> str:
    start = token.text.find('"')
    if token.kind == "raw":
        hashes = token.text[:start].count("#")
        return token.text[start + 1 : -(hashes + 1)]
    text = token.text[start + 1 : -1]
    return text.replace(r"\n", "\n").replace(r"\t", "\t").replace(r"\"", '"')


def rust_violations(source: str, constants: set[str] | None = None) -> list[int]:
    tokens = rust_tokens(source)
    ignored = set()
    parents: dict[int, tuple[int, ...]] = {}
    stack = []
    for index, token in enumerate(tokens):
        parents[index] = tuple(stack)
        if token.text in {"(", "[", "{"}:
            stack.append(index)
        elif token.text in {")", "]", "}"} and stack:
            stack.pop()
        if token.text == "#" and index + 1 < len(tokens) and tokens[index + 1].text == "[":
            end = closing(tokens, index + 1)
            attribute = [value.text for value in tokens[index + 2 : end]]
            if "test" in attribute:
                body = end + 1
                while body < len(tokens) and tokens[body].text not in {"{", ";"}:
                    body += 1
                if body < len(tokens):
                    ignored.update(
                        range(
                            index,
                            closing(tokens, body) + 1 if tokens[body].text == "{" else body + 1,
                        )
                    )
        if (
            token.text == "const"
            and index + 1 < len(tokens)
            and tokens[index + 1].text in (constants or set())
        ):
            end = next(
                (i for i in range(index, len(tokens)) if tokens[i].text == ";"), len(tokens) - 1
            )
            ignored.update(range(index, end + 1))
    found = set()
    for index, token in enumerate(tokens):
        if index in ignored:
            continue
        macro = (
            token.text in {"format", "concat"}
            and index + 2 < len(tokens)
            and tokens[index + 1].text == "!"
        )
        push = token.text == "push_str" and index > 0 and tokens[index - 1].text == "."
        if not (macro or push):
            continue
        opening = index + (2 if macro else 1)
        if opening >= len(tokens) or tokens[opening].text not in {"(", "[", "{"}:
            continue
        diagnostic = False
        for parent in parents[index]:
            name_index = parent - 2 if parent and tokens[parent - 1].text == "!" else parent - 1
            name = tokens[name_index].text if name_index >= 0 else ""
            diagnostic |= name in {
                "Err",
                "new_err",
                "ok_or_else",
                "map_err",
                "panic",
                "assert",
                "assert_eq",
                "assert_ne",
                "debug",
                "info",
                "warn",
                "warning",
                "error",
                "trace",
            } or name.endswith(("_error", "_diagnostic"))
        before = [value.text for value in tokens[max(0, index - 2) : index]]
        diagnostic |= before in [["message", ":"], ["reason", ":"], ["help", ":"]]
        if diagnostic:
            continue
        end = closing(tokens, opening)
        text = "".join(
            literal(value) for value in tokens[opening + 1 : end] if value.kind in {"raw", "string"}
        )
        if constructs_typeql(text):
            found.add(source.count("\n", 0, token.start) + 1)
    return sorted(found)


def main() -> int:
    allowlist = json.loads((ROOT / "scripts/ci/typeql_text_allowlist.json").read_text())
    legacy = allowlist["legacy_emitters"]
    assert all(entry["remove_in"] == "11.30" and entry["reason"] for entry in legacy.values())
    files = subprocess.check_output(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "type_bridge/**/*.py",
            "type-bridge-core/crates/**/*.rs",
        ],
        cwd=ROOT,
        text=True,
    ).splitlines()
    test_modules = set()
    for relative in files:
        if not relative.endswith(".rs"):
            continue
        path = ROOT / relative
        source = path.read_text()
        for match in re.finditer(
            r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", source
        ):
            directory = (
                path.parent
                if path.name in {"lib.rs", "main.rs", "mod.rs"}
                else path.with_suffix("")
            )
            test_modules.update({directory / (match[1] + ".rs"), directory / match[1] / "mod.rs"})
    found = []
    for relative in files:
        if (
            relative.startswith(ADAPTER)
            or relative in legacy
            or "/tests/" in relative
            or ROOT / relative in test_modules
        ):
            continue
        if relative.endswith(".rs") and "/src/" not in relative:
            continue
        source = (ROOT / relative).read_text()
        constants = {
            entry.split("#", 1)[1]
            for entry in allowlist["constants"]
            if entry.startswith(relative + "#")
        }
        lines = (
            rust_violations(source, constants)
            if relative.endswith(".rs")
            else python_violations(source)
        )
        found.extend(f"{relative}:{line}" for line in lines)
    if found:
        print("TypeQL construction outside the AST adapter:\n" + "\n".join(found))
        return 1
    print("TypeQL text construction boundary: passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
