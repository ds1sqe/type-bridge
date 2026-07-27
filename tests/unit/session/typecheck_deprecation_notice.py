"""Positive Pyright fixture for the public deprecation-notice default."""

from typing import assert_type

import type_bridge_core

from type_bridge import TypeDBServerDeprecationWarning

assert_type(type_bridge_core.typedb_server_deprecation_notice(), str | None)
assert_type(type_bridge_core.typedb_server_deprecation_notice(None), str | None)
assert_type(type_bridge_core.typedb_server_deprecation_notice("3.12.1"), str | None)
assert_type(TypeDBServerDeprecationWarning.code, str)
