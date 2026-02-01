"""Type mapping utilities for code generation."""

from collections.abc import Mapping

# Mapping from TypeDB value types to Python types
TYPEDB_TO_PYTHON: Mapping[str, str] = {
    "string": "str",
    "integer": "int",
    "long": "int",  # TypeDB long maps to Python int
    "double": "float",
    "boolean": "bool",
    "date": "date",
    "datetime": "datetime",
    "datetime-tz": "datetime",
    "decimal": "Decimal",
    "duration": "timedelta",
}

# Mapping from TypeDB value types to type-bridge attribute classes
TYPEDB_TO_BRIDGE: Mapping[str, str] = {
    "string": "String",
    "integer": "Integer",
    "long": "Integer",  # TypeDB long maps to type-bridge Integer
    "double": "Double",
    "datetime": "DateTime",
    "datetime-tz": "DateTimeTZ",
    "date": "Date",
    "duration": "Duration",
    "boolean": "Boolean",
    "decimal": "Decimal",
}
