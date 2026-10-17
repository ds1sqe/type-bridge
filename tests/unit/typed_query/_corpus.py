"""Shared typed-query corpus lookups for native-only unit tests."""

import json
from functools import cache
from pathlib import Path

_CORPUS = Path(__file__).parents[2] / "contracts" / "typed_query" / "corpus-v1.json"


@cache
def _corpus_errors() -> dict[str, tuple[str, str]]:
    corpus = json.loads(_CORPUS.read_text(encoding="utf-8"))
    return {
        case["id"]: (case["expected"]["error_category"], case["expected"]["error_code"])
        for case in corpus["cases"]
        if case["expected"]["outcome"] == "error"
    }


def corpus_error(case_id: str) -> tuple[str, str]:
    """Return the shared category/code pair for one public runtime case."""
    return _corpus_errors()[case_id]
