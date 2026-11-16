"""One killable raw-oracle process; the parent owns database cleanup."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from fixture import ORIGINAL_SORTED_RAW_QUERY, SORTED_RAW_QUERY, width_query
from typedb.driver import Credentials, TransactionType, TypeDB

from type_bridge import create_driver_options


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle", choices=["fetch", "sorted", "original"], required=True)
    parser.add_argument("--query-file", type=Path)
    args = parser.parse_args()
    with TypeDB.driver(
        os.environ["TYPEDB_ADDRESS"],
        Credentials("admin", "password"),
        create_driver_options(is_tls_enabled=False),
    ) as driver:
        with driver.transaction(
            os.environ["TYPE_BRIDGE_COMPLEX_DATABASE"], TransactionType.READ
        ) as tx:
            query = (
                ORIGINAL_SORTED_RAW_QUERY
                if args.oracle == "original"
                else SORTED_RAW_QUERY
                if args.oracle == "sorted"
                else width_query(int(os.environ.get("TYPE_BRIDGE_COMPLEX_WIDTH", "12")))
            )
            if args.query_file is not None:
                query = args.query_file.read_text()
            raw = list(tx.query(query).resolve().as_concept_documents())
    print(json.dumps(sorted([row["identifier"], row.get("note")] for row in raw)))


if __name__ == "__main__":
    main()
