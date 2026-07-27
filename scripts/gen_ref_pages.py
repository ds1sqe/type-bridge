"""Generate API reference pages from type_bridge source modules."""

from pathlib import Path

import mkdocs_gen_files

nav = mkdocs_gen_files.Nav()

src = Path("type_bridge")

EXCLUDE_PARTS = {
    "__pycache__",
    "__main__",
    "templates",
}

for path in sorted(src.rglob("*.py")):
    if any(part in EXCLUDE_PARTS for part in path.parts):
        continue
    if path.name.startswith("_") and path.name != "__init__.py":
        continue

    module_path = path.with_suffix("")
    doc_path = path.relative_to(src).with_suffix(".md")
    full_doc_path = Path("reference", doc_path)

    parts = tuple(module_path.parts)
    if parts[-1] == "__init__":
        parts = parts[:-1]
        doc_path = doc_path.with_name("index.md")
        full_doc_path = full_doc_path.with_name("index.md")

    if not parts:
        continue

    identifier = ".".join(parts)

    nav[parts] = doc_path.as_posix()

    with mkdocs_gen_files.open(full_doc_path, "w") as fd:
        fd.write(f"# `{identifier}`\n\n")
        fd.write(f"::: {identifier}\n")

    mkdocs_gen_files.set_edit_path(full_doc_path, path.as_posix())

with mkdocs_gen_files.open("reference/SUMMARY.md", "w") as nav_file:
    nav_file.writelines(nav.build_literate_nav())

# Copy CHANGELOG.md into docs at build time. The repo file links doc pages as
# `docs/guide/...` (GitHub-relative); the copy lives at the docs-site root, so
# rewrite those links to site-relative `guide/...` or strict mode breaks.
changelog = Path("CHANGELOG.md")
if changelog.exists():
    with mkdocs_gen_files.open("changelog.md", "w") as dst:
        dst.write(changelog.read_text().replace("(docs/guide/", "(guide/"))
