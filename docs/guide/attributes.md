# Attributes

Complete reference for TypeBridge's attribute types and value types.

## Overview

In TypeBridge, **attributes are independent types** that map directly to TypeDB's attribute system. Each attribute has a value type (string, integer, double, etc.) and can be owned by multiple entity or relation types.

## Attribute Base Class

All attributes inherit from the abstract `Attribute` base class:

```python
from abc import ABC
from typing import ClassVar

class Attribute(ABC):
    """Base class for all TypeDB attributes."""
    value_type: ClassVar[str]  # TypeDB value type

    @classmethod
    def get_attribute_name(cls) -> str:
        """Returns the TypeDB attribute name (lowercase class name)."""

    @classmethod
    def to_schema_definition(cls) -> str:
        """Generates TypeQL schema: 'attribute name, value string;'"""
```

## Concrete Attribute Types

TypeBridge provides all 9 TypeDB value types:

### String

Text values of any length.

```python
from type_bridge import String

class Name(String):
    pass

class Email(String):
    pass

# Usage
person = Person(name=Name("Alice"), email=Email("alice@example.com"))
```

**TypeQL**: `attribute name, value string;`

#### Special Characters and Escaping

TypeBridge automatically handles special characters in string values when generating TypeQL queries. You never need to manually escape strings in your Python code.

**Automatic escaping behavior:**

```python
# Quotes are escaped automatically
person = Person(
    name=Name("Alice"),
    tags=[Tag('skill "Python"'), Tag('role "Engineer"')]
)
# Generates TypeQL: has Tag "skill \"Python\"", has Tag "role \"Engineer\""

# Backslashes are escaped automatically
fileset = FileSet(
    name=Name("Files"),
    paths=[Path("C:\\Users\\Alice"), Path("D:\\Projects\\App")]
)
# Generates TypeQL: has Path "C:\\Users\\Alice", has Path "D:\\Projects\\App"

# Mixed escaping works correctly
doc = Document(
    name=Name("README"),
    notes=[Description(r'Path: "C:\Program Files\App"')]
)
# Generates TypeQL: has Description "Path: \"C:\\Program Files\\App\""

# Single quotes don't need escaping (TypeQL uses double quotes)
person = Person(
    name=Name("Bob"),
    tags=[Tag("it's"), Tag("can't"), Tag("won't")]
)
# Single quotes preserved as-is: has Tag "it's", has Tag "can't"

# Unicode is fully supported
article = Article(
    name=Name("Article"),
    tags=[Tag("café"), Tag("日本語"), Tag("emoji😀")]
)
# Unicode preserved without escaping
```

**Escaping rules:**

- Backslashes (`\`) are escaped to `\\`
- Double quotes (`"`) are escaped to `\"`
- Single quotes (`'`) are NOT escaped (TypeQL uses double quotes for strings)
- Unicode characters are preserved without escaping
- Escape order: backslashes first, then quotes (important for correct output)

**Note:** Escaping happens automatically during query generation. You never need to manually escape strings in your Python code - just pass them as normal Python strings.

### Integer

64-bit signed integers (renamed from `Long` in TypeDB 2.x).

```python
from type_bridge import Integer

class Age(Integer):
    pass

class Count(Integer):
    pass

# Usage
person = Person(age=Age(30), count=Count(42))
```

**TypeQL**: `attribute age, value integer;`

**Range**: -2^63 to 2^63 - 1

### Double

IEEE 754 floating-point numbers.

```python
from type_bridge import Double

class Score(Double):
    pass

class Temperature(Double):
    pass

# Usage
result = Result(score=Score(95.5), temperature=Temperature(37.2))
```

**TypeQL**: `attribute score, value double;`

**Use for**: Scientific calculations, measurements, approximate values

### Decimal

High-precision fixed-point numbers with 19 decimal digits of precision.

```python
from type_bridge import Decimal
from decimal import Decimal as DecimalType

class Price(Decimal):
    pass

class AccountBalance(Decimal):
    pass

# Usage - use string for exact precision (recommended)
product = Product(price=Price("19.99"))
account = Account(balance=AccountBalance("1234.567890123456789"))
```

**TypeQL**: `attribute price, value decimal;` (values use `dec` suffix in queries)

**Range**: -2^63 to 2^63 - 10^-19

**Precision**: 19 decimal digits after decimal point

**Use for**: Financial calculations, monetary values, exact decimal representation

### Double vs Decimal

Choose the right numeric type for your use case:

| Feature     | Double                   | Decimal                   |
| ----------- | ------------------------ | ------------------------- |
| Precision   | Approximate (IEEE 754)   | Exact (19 decimal digits) |
| Use cases   | Scientific, measurements | Financial, monetary       |
| Performance | Faster                   | Slightly slower           |
| Range       | Larger                   | Smaller but sufficient    |
| Example     | `95.5`, `37.2`           | `"19.99"`, `"1234.56"`    |

```python
# ✅ Use Double for scientific data
class Temperature(Double):
    pass

# ✅ Use Decimal for financial data
class Price(Decimal):
    pass
```

### Boolean

True/false values.

```python
from type_bridge import Boolean

class IsActive(Boolean):
    pass

class IsVerified(Boolean):
    pass

# Usage
user = User(is_active=IsActive(True), is_verified=IsVerified(False))
```

**TypeQL**: `attribute is_active, value boolean;`

### Date

Date-only values without time information.

```python
from type_bridge import Date
from datetime import date

class BirthDate(Date):
    pass

class PublishDate(Date):
    pass

# Usage
person = Person(birth_date=BirthDate(date(1990, 5, 15)))
book = Book(publish_date=PublishDate(date(2024, 3, 30)))
```

**TypeQL**: `attribute birth_date, value date;`

**Format**: ISO 8601 date (YYYY-MM-DD)

**Range**: January 1, 262144 BCE to December 31, 262142 CE

**Use for**: Birth dates, publish dates, deadlines, anniversaries

### DateTime

Naive datetime without timezone information.

```python
from type_bridge import DateTime
from datetime import datetime

class CreatedAt(DateTime):
    pass

class LoggedAt(DateTime):
    pass

# Usage
event = Event(created_at=CreatedAt(datetime(2024, 3, 30, 10, 30, 45)))
```

**TypeQL**: `attribute created_at, value datetime;`

**Format**: ISO 8601 datetime (YYYY-MM-DDTHH:MM:SS)

**Use for**: Timestamps where timezone context is implicit or unnecessary

### DateTimeTZ

Timezone-aware datetime with explicit timezone information.

```python
from type_bridge import DateTimeTZ
from datetime import datetime, timezone

class UpdatedAt(DateTimeTZ):
    pass

class ScheduledAt(DateTimeTZ):
    pass

# Usage
record = Record(updated_at=UpdatedAt(datetime(2024, 3, 30, 10, 30, 45, tzinfo=timezone.utc)))
```

**TypeQL**: `attribute updated_at, value datetime-tz;`

**Format**: ISO 8601 with timezone (YYYY-MM-DDTHH:MM:SS±HH:MM or with IANA TZ identifier)

**Use for**: Distributed systems, events across timezones, UTC timestamps

### Date/DateTime/DateTimeTZ Comparison

Choose the right temporal type:

| Type       | Time Info | Timezone | Use Cases                               |
| ---------- | --------- | -------- | --------------------------------------- |
| Date       | No        | No       | Birth dates, deadlines, anniversaries   |
| DateTime   | Yes       | No       | Local events, single-timezone systems   |
| DateTimeTZ | Yes       | Yes      | Global events, distributed systems, UTC |

### Duration

ISO 8601 duration for calendar-aware time spans.

```python
from type_bridge import Duration
from datetime import timedelta

class EventCadence(Duration):
    pass

class SessionLength(Duration):
    pass

# ISO 8601 format
hourly = EventCadence("PT1H")                      # 1 hour
daily = EventCadence("P1D")                        # 1 day
monthly = EventCadence("P1M")                      # 1 month
complex = EventCadence("P1Y2M3DT4H5M6.789S")      # 1 year, 2 months, 3 days, 4:05:06.789

# From Python timedelta
from_td = EventCadence(timedelta(hours=2, minutes=30))  # PT2H30M
```

**TypeQL**: `attribute event_cadence, value duration;`

**Format**: ISO 8601 duration (`P[years]Y[months]M[days]DT[hours]H[minutes]M[seconds]S`)

**Storage**: 32-bit months, 32-bit days, 64-bit nanoseconds

**Properties**:

- **Calendar-aware**: `P1D` ≠ `PT24H`, `P1M` varies by month
- **Partially ordered**: Cannot directly compare `P1M` vs `P30D`

**Use for**: Recurring events, calendar-relative time spans, time intervals with months/years

## DateTime and DateTimeTZ Conversions

TypeBridge provides conversion methods between naive and timezone-aware datetimes:

### Add Timezone to DateTime

```python
from datetime import datetime, timezone, timedelta

naive_dt = DateTime(datetime(2024, 1, 15, 10, 30, 45))

# Implicit: add system timezone
aware_dt = naive_dt.add_timezone()

# Explicit: add specific timezone
jst = timezone(timedelta(hours=9))
aware_jst = naive_dt.add_timezone(jst)  # Add JST timezone
aware_utc = naive_dt.add_timezone(timezone.utc)  # Add UTC timezone
```

### Strip Timezone from DateTimeTZ

```python
from datetime import datetime, timezone, timedelta

aware_dt = DateTimeTZ(datetime(2024, 1, 15, 10, 30, 45, tzinfo=timezone.utc))

# Implicit: just strip timezone
naive_dt = aware_dt.strip_timezone()

# Explicit: convert to timezone first, then strip
jst = timezone(timedelta(hours=9))
naive_jst = aware_dt.strip_timezone(jst)  # Convert to JST, then strip
```

**Conversion semantics**:

- `DateTime.add_timezone(tz=None)`: If tz is None, adds system timezone; otherwise adds specified timezone
- `DateTimeTZ.strip_timezone(tz=None)`: If tz is None, strips timezone as-is; otherwise converts to tz first, then strips

## Duration Arithmetic

Duration supports calendar-aware arithmetic with Date, DateTime, and DateTimeTZ:

```python
from datetime import date, datetime, timezone

# Add duration to date
release = Date(date(2024, 1, 31))
one_month = Duration("P1M")
result = release + one_month  # Date(2024-02-29) (leap year, last day of month)

# Subtract duration from date
deadline = Date(date(2024, 3, 31))
result = deadline - one_month  # Date(2024-02-29)

# Add duration to datetime
start = DateTime(datetime(2024, 1, 31, 14, 0, 0))
result = start + one_month  # Feb 29, 2024 (leap year, last day of month)

# Add duration to timezone-aware datetime
start_utc = DateTimeTZ(datetime(2024, 1, 31, 14, 0, 0, tzinfo=timezone.utc))
result_utc = start_utc + one_month  # Respects timezone

# Duration arithmetic
d1 = Duration("P1M")
d2 = Duration("P15D")
total = d1 + d2  # P1M15D
```

**Important notes**:

- Addition order matters: `P1M + P1D` ≠ `P1D + P1M` (calendar arithmetic)
- Month addition respects calendar (Jan 31 + 1 month = Feb 29 if leap year)
- Duration with DateTimeTZ respects DST and timezone changes
- Reverse addition works: `Duration("P1D") + Date(...)` returns a `Date`

## Configuring Attribute Type Names

By default, attribute type names match the Python class name exactly (e.g., `class PersonName` → `PersonName`). You can override this using `AttributeFlags`:

### Using AttributeFlags.name

Explicitly set the TypeDB attribute type name:

```python
from type_bridge import AttributeFlags, String

class Name(String):
    flags = AttributeFlags(name="name")

# TypeDB: attribute name, value string;
```

**Use cases**:

- Interop with existing TypeDB schemas using lowercase names
- Match legacy naming conventions
- Simplify migration from manual TypeQL

### Using AttributeFlags.case

Apply case formatting to the class name:

```python
from type_bridge import AttributeFlags, String, TypeNameCase

class PersonName(String):
    flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)
# TypeDB: attribute person_name, value string;

class UserEmail(String):
    flags = AttributeFlags(case=TypeNameCase.LOWERCASE)
# TypeDB: attribute useremail, value string;
```

**Available cases**:

- `TypeNameCase.CLASS_NAME` - Preserve as-is (default)
- `TypeNameCase.LOWERCASE` - All lowercase
- `TypeNameCase.SNAKE_CASE` - snake_case conversion
- `TypeNameCase.KEBAB_CASE` - kebab-case conversion

### Documentation and Metadata (@doc/@meta)

TypeDB 3.12+ supports schema-level documentation and metadata annotations
on the attribute type:

```python
class Email(String):
    flags = AttributeFlags(
        name="email",
        doc="A contact email address.",
        meta={"pii": "true"},
    )

# TypeDB: attribute email @doc("A contact email address.") @meta("pii", "true"), value string;
```

Annotations require a TypeDB 3.12+ server: schema application is
version-gated and raises a versioned error against older servers (see the
[schema guide](schema.md#schema-annotations-and-server-versions)).

### Priority Order

When determining the attribute type name, TypeBridge uses this priority:

1. **`flags.name`** (highest) - Explicit name override
2. **`attr_name`** - Class-level `attr_name = "..."`
3. **`flags.case`** - Case formatting from flags
4. **`case`** - Class-level `case = TypeNameCase.SNAKE_CASE`
5. **Default** - Preserve class name as-is

Example showing all options:

```python
# Option 1: Explicit name (highest priority)
class Name(String):
    flags = AttributeFlags(name="person_name")

# Option 2: Class-level attr_name
class Email(String):
    attr_name = "email_address"

# Option 3: Case formatting via flags
class UserStatus(String):
    flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)  # -> user_status

# Option 4: Class-level case
class CompanyName(String):
    case = TypeNameCase.LOWERCASE  # -> companyname

# Option 5: Default (preserve class name)
class Age(Integer):
    pass  # -> Age
```

## Value Constraints

TypeBridge supports TypeDB's value constraint annotations. These provide two layers of validation:

1. **Python-side**: Immediate validation with clear error messages
2. **TypeDB-side**: Database-level enforcement via schema annotations

### @range - Numeric Range Constraints

Constrain `Integer` or `Double` values to a specific range:

```python
from typing import ClassVar
from type_bridge import Integer, Double

class Age(Integer):
    """Age must be between 0 and 150."""
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

class Temperature(Double):
    """Temperature in Celsius, -50 to 50."""
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("-50.0", "50.0")

# Open-ended ranges
class Score(Integer):
    """Non-negative scores (min only)."""
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", None)

class Priority(Integer):
    """Priority up to 10 (max only)."""
    range_constraint: ClassVar[tuple[str | None, str | None]] = (None, "10")
```

**Validation behavior:**

```python
Age(30)    # ✅ Valid
Age(-1)    # ❌ ValueError: Age value -1 is below minimum 0
Age(200)   # ❌ ValueError: Age value 200 is above maximum 150
```

**Generated TypeQL:**

```typeql
attribute Age, value integer @range(0..150);
attribute Temperature, value double @range(-50.0..50.0);
attribute Score, value integer @range(0..);
attribute Priority, value integer @range(..10);
```

### @regex - Pattern Constraints

Constrain `String` values to match a regex pattern:

```python
from typing import ClassVar
from type_bridge import String

class Email(String):
    """Must be a valid email format."""
    regex: ClassVar[str] = r"^[a-z]+@[a-z]+\.[a-z]+$"

class PhoneNumber(String):
    """International phone format."""
    regex: ClassVar[str] = r"^\+?[0-9]{10,14}$"
```

**Generated TypeQL:**

```typeql
attribute Email, value string @regex("^[a-z]+@[a-z]+\.[a-z]+$");
```

### @values - Enumerated Values

Constrain `String` to a set of allowed values:

```python
from typing import ClassVar
from type_bridge import String

class Status(String):
    """Only these status values are allowed."""
    allowed_values: ClassVar[tuple[str, ...]] = ("active", "inactive", "pending")

class Priority(String):
    """Priority levels."""
    allowed_values: ClassVar[tuple[str, ...]] = ("low", "medium", "high", "critical")
```

**Generated TypeQL:**

```typeql
attribute Status, value string @values("active", "inactive", "pending");
```

### @independent - Standalone Attributes

Allow attributes to exist without being owned by an entity or relation:

```python
from type_bridge import String, Integer

class Language(String):
    """Can exist without an owner."""
    independent = True

class GlobalCounter(Integer):
    """Shared counter that can be queried directly."""
    independent = True
```

**Usage:**

```python
# Independent attributes can be inserted directly via TypeQL
# without being owned by an entity
insert_query = 'insert $lang isa Language "English";'

# Can also be owned by entities as normal
class Document(Entity):
    name: Name = Flag(Key)
    language: Language | None = None
```

**Generated TypeQL:**

```typeql
attribute Language @independent, value string;
attribute GlobalCounter @independent, value integer;
```

**Combining with other annotations:**

```python
class SharedScore(Integer):
    """Independent attribute with range constraint."""
    independent = True
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "100")

# Generated: attribute SharedScore @independent, value integer @range(0..100);
```

**Check if attribute is independent:**

```python
Language.is_independent()  # True
Name.is_independent()      # False
```

### Schema Synchronization

When using `SchemaManager.sync_schema()`, these constraints are automatically included in the generated TypeQL schema, ensuring TypeDB enforces them at the database level:

```python
from type_bridge import SchemaManager, Database

db = Database(address="localhost:1729", database="mydb")
schema_manager = SchemaManager(db)
schema_manager.register(Person)  # Person owns Age with range_constraint
schema_manager.sync_schema()     # @range is included in TypeDB schema
```

## List Attributes

TypeDB supports *list-ordered* attribute ownership via `owns attr[]`.  An entity or
relation may declare that it holds an **ordered, possibly-distinct** list of attribute
values rather than an unordered set.

### Declaring a list attribute

Use `Flag(Ordered)` (ordered list, duplicates allowed) or `Flag(Ordered, Distinct)`
(ordered list, all values unique):

```python
from type_bridge import Entity, Flag, Key, Ordered, Distinct, String, TypeFlags

class Tag(String):
    pass

class Title(String):
    pass

class Article(Entity):
    flags = TypeFlags(name="article")
    title: Title = Flag(Key)
    # ordered list of tags, no duplicates
    tag: list[Tag] = Flag(Ordered, Distinct)
```

Generated TypeQL:

```typeql
entity article, owns title @key, owns tag[] @distinct;
```

Using `Flag(Ordered)` alone (without `Distinct`) omits the `@distinct` annotation:

```python
class Playlist(Entity):
    flags = TypeFlags(name="playlist")
    entries: list[Tag] = Flag(Ordered)  # duplicates allowed
```

Generated TypeQL:

```typeql
entity playlist, owns tag[];
```

### Validation

`Flag(Distinct)` without `Flag(Ordered)` raises `ValueError` immediately — `@distinct`
is only valid on a list attribute in TypeDB:

```python
# This raises ValueError at class definition time:
class Bad(Entity):
    flags = TypeFlags(name="bad")
    tag: Tag = Flag(Distinct)   # ValueError: requires Ordered
```

### Engine caveat — REP256

As of TypeDB 3.x, instance-level list operations (inserting or reading list attribute
values) are **not yet implemented** in the engine (tracked as REP256).  Schema-side
declarations (`sync_schema()`) are accepted; any attempt to insert a list attribute value is
rejected with an engine error.

This means:

- You **can** define models with `Flag(Ordered, Distinct)` and call `sync_schema()`.
- You **cannot** yet insert, read, or filter by list attribute values at runtime.

Update this section when a TypeDB release implements list instances (the pinned integration test will fail and point here).

## Best Practices

### 1. Create Distinct Attribute Types

Each semantic field should use a distinct attribute type:

```python
# ✅ CORRECT: Distinct types
class CreatedStamp(DateTime):
    pass

class ModifiedStamp(DateTime):
    pass

class Issue(Entity):
    created: CreatedStamp
    modified: ModifiedStamp

# ❌ WRONG: Duplicate attribute types
class TimeStamp(DateTime):
    pass

class Issue(Entity):
    created: TimeStamp   # Error: duplicate attribute type
    modified: TimeStamp  # TypeDB sees only one ownership
```

### 2. Use Literal Types for Type Safety

Combine attributes with `Literal` for IDE autocomplete:

```python
from typing import Literal

class Status(String):
    pass

# Type checker provides autocomplete for these values
status: Literal["active", "inactive", "pending"] | Status
```

### 3. Choose the Right Numeric Type

- Use `Integer` for whole numbers
- Use `Double` for scientific/approximate calculations
- Use `Decimal` for financial/exact calculations

### 4. Choose the Right Temporal Type

- Use `Date` for date-only values
- Use `DateTime` for naive timestamps
- Use `DateTimeTZ` for timezone-aware timestamps
- Use `Duration` for calendar-aware time spans

## See Also

- [Entities](entities.md) - How entities own attributes
- [Relations](relations.md) - How relations own attributes
- [Abstract Types](abstract-types.md) - Abstract attribute types and inheritance patterns
- [Cardinality](cardinality.md) - Cardinality constraints for attributes
- [Validation](validation.md) - Type validation and Pydantic integration
