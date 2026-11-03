"""Python SDK for generated TypeBridge applications."""

from __future__ import annotations

from type_bridge.failure import CommitConflict, CommitOutcomeUnknown, CommitRejected, Failure
from type_bridge.plan import Plan, PlanBuilder, PlanExplanation, StagedPlan, StagedResultRef
from type_bridge.proxy import ProxyDatabase, ProxyError
from type_bridge.session import (
    Connection,
    Database,
    TransactionContext,
)
from type_bridge.typedb_driver import Credentials, TransactionType, TypeDB, create_driver_options
from type_bridge.write_result import BatchWriteResult, WriteResult

__version__ = "2.3.2"


__all__ = [
    "BatchWriteResult",
    "WriteResult",
    "Failure",
    "CommitConflict",
    "CommitRejected",
    "CommitOutcomeUnknown",
    # Database and session
    "Connection",
    "Database",
    "TransactionContext",
    # Typed plans for staged transactions
    "Plan",
    "PlanExplanation",
    "PlanBuilder",
    "StagedPlan",
    "StagedResultRef",
    # TypeDB driver (re-exported for convenience)
    "Credentials",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
    # Proxy
    "ProxyDatabase",
    "ProxyError",
]
