"""Simple data builders for test fixtures.

These builders create test entities/relations with sensible defaults.
Override specific fields as needed.
"""

from uuid import uuid4


def unique_suffix() -> str:
    """Generate a short unique suffix for test data."""
    return uuid4().hex[:6]


def make_name(prefix: str = "Test") -> str:
    """Generate a unique name string."""
    return f"{prefix}-{unique_suffix()}"


def make_email(name: str | None = None) -> str:
    """Generate a unique email address."""
    name = name or f"user-{unique_suffix()}"
    return f"{name.lower().replace(' ', '-')}@test.com"


def make_isbn() -> str:
    """Generate a fake ISBN-13."""
    import random

    # Generate 12 random digits
    digits = [random.randint(0, 9) for _ in range(12)]
    # Calculate check digit (simplified)
    check = sum(d * (1 if i % 2 == 0 else 3) for i, d in enumerate(digits)) % 10
    check = (10 - check) % 10
    return "".join(str(d) for d in digits) + str(check)


# Type-specific builders to be used after importing generated models
# Example usage:
#
#   from tests.utils.data_builders import make_name, make_email
#   from my_models import Person, Name, Email
#
#   person = Person(
#       name=Name(make_name("Alice")),
#       email=Email(make_email("alice"))
#   )
